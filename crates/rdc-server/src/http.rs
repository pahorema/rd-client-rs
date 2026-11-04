//! Router assembly, path handling, API errors, and embedded web assets.

use std::{any::Any, fmt, sync::Arc};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{DefaultBodyLimit, Request},
    http::{StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{MethodRouter, get},
};
use rust_embed::RustEmbed;
use tower_http::catch_panic::CatchPanicLayer;

use crate::{config::normalize_base_path, logging::LogControl};

const MAX_BODY_SIZE: usize = 30_000_000;

/// Plain-text handler error retaining its body under every application prefix.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    /// Construct a response with a specific status and unquoted text body.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

/// Request extension containing the matched mount prefix, or an empty root prefix.
/// Authentication handlers can use this value as their cookie Path (append `/`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestBasePath(pub String);

/// Assemble handlers with shared application state and process middleware.
///
/// Register API paths through [`Self::route`]; literal segments are canonicalized
/// while captured path parameters retain their exact bytes. Call [`Self::build`]
/// once, supplying any cloneable state used by the registered handlers.
#[derive(Debug)]
#[must_use]
pub struct AppBuilder<S = ()> {
    router: Router<S>,
    patterns: Vec<String>,
    base_path: String,
    logs: LogControl,
}

impl<S> AppBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    /// Start an application with embedded assets and no API endpoints.
    pub fn new(base_path: &str, logs: LogControl) -> Self {
        Self {
            router: Router::new()
                .route("/add", get(magnet_redirect))
                .fallback(static_file),
            patterns: Vec::new(),
            base_path: normalize_base_path(base_path),
            logs,
        }
    }

    /// Register an endpoint; API literal path segments are matched case-insensitively.
    /// Captures use axum syntax (`{id}` or `{*path}`), and are never lowercased.
    pub fn route(mut self, path: &str, methods: MethodRouter<S>) -> Self {
        let canonical = canonical_pattern(path);
        self.router = self.router.route(&canonical, methods);
        self.patterns.push(canonical);
        self
    }

    /// Merge an existing router whose API literal segments are already lowercase.
    /// Supply every API path template so case normalization preserves captures.
    pub fn merge(mut self, router: Router<S>, paths: &[&str]) -> Self {
        self.router = self.router.merge(router);
        self.patterns
            .extend(paths.iter().map(|path| canonical_pattern(path)));
        self
    }

    /// Finish assembly with handler state; returned router is ready for `axum::serve`.
    pub fn build(mut self, state: S) -> Router {
        self.patterns.sort_by_cached_key(|pattern| {
            std::cmp::Reverse(
                pattern
                    .split('/')
                    .filter(|part| !part.starts_with('{'))
                    .map(str::len)
                    .sum::<usize>(),
            )
        });
        let inner = self
            .router
            .with_state(state)
            .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
            .layer(CatchPanicLayer::custom(panic_response));
        let plumbing = Arc::new(Plumbing {
            patterns: self.patterns,
            base_path: self.base_path,
            logs: self.logs,
        });
        // Route only after preprocessing the URI; router layers run after route matching.
        Router::new()
            .fallback_service(inner)
            .layer(middleware::from_fn(move |request, next| {
                preprocess(request, next, plumbing.clone())
            }))
    }
}

struct Plumbing {
    patterns: Vec<String>,
    base_path: String,
    logs: LogControl,
}

async fn preprocess(request: Request, next: Next, plumbing: Arc<Plumbing>) -> Response {
    let original_uri = request.uri().clone();
    let response = dispatch(request, next, &plumbing).await;
    if !response.status().is_success() {
        tracing::warn!(status = response.status().as_u16(), path = %plumbing.logs.redact(original_uri.path()), "HTTP response");
    }
    response
}

async fn dispatch(mut request: Request, next: Next, plumbing: &Plumbing) -> Response {
    let original_path = request.uri().path().to_owned();
    let path = if !plumbing.base_path.is_empty() && original_path == plumbing.base_path {
        let query = request
            .uri()
            .query()
            .map_or(String::new(), |query| format!("?{query}"));
        return Redirect::temporary(&format!("{}/{query}", plumbing.base_path)).into_response();
    } else if !plumbing.base_path.is_empty()
        && original_path
            .strip_prefix(&plumbing.base_path)
            .is_some_and(|suffix| suffix.starts_with('/'))
    {
        request
            .extensions_mut()
            .insert(RequestBasePath(plumbing.base_path.clone()));
        &original_path[plumbing.base_path.len()..]
    } else {
        request
            .extensions_mut()
            .insert(RequestBasePath(String::new()));
        &original_path
    };
    let api = is_api(path);
    let canonical = normalize_api_path(path, &plumbing.patterns);
    let path_query = request
        .uri()
        .query()
        .map_or_else(|| canonical.clone(), |query| format!("{canonical}?{query}"));
    let mut uri = request.uri().clone().into_parts();
    let Ok(path_query) = path_query.parse() else {
        return ApiError::new(StatusCode::BAD_REQUEST, "Invalid request path").into_response();
    };
    uri.path_and_query = Some(path_query);
    let Ok(uri) = Uri::from_parts(uri) else {
        return ApiError::new(StatusCode::BAD_REQUEST, "Invalid request URI").into_response();
    };
    *request.uri_mut() = uri;
    if tracing::enabled!(tracing::Level::DEBUG) && should_log(&canonical) {
        request = match log_request(request, &plumbing.logs).await {
            Ok(request) => request,
            Err(error) => return error.into_response(),
        };
    }
    let mut response = next.run(request).await;
    if api && response.status() == StatusCode::UNAUTHORIZED {
        *response.status_mut() = StatusCode::FORBIDDEN;
    }
    response
}

fn canonical_pattern(path: &str) -> String {
    if !is_api(path) {
        return path.to_owned();
    }
    path.split('/')
        .map(|part| {
            if part.starts_with('{') {
                part.to_owned()
            } else {
                part.to_ascii_lowercase()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_api_path(path: &str, patterns: &[String]) -> String {
    if !is_api(path) {
        return path.to_owned();
    }
    let segment_count = path.split('/').count();
    for pattern in patterns {
        let literal_count = pattern.split('/').count();
        let wildcard = pattern
            .rsplit('/')
            .next()
            .is_some_and(|part| part.starts_with("{*"));
        if (!wildcard && literal_count != segment_count)
            || (wildcard && segment_count < literal_count)
        {
            continue;
        }
        if pattern
            .split('/')
            .zip(path.split('/'))
            .all(|(literal, value)| literal.starts_with('{') || literal.eq_ignore_ascii_case(value))
        {
            let mut result = String::with_capacity(path.len());
            let mut literals = pattern.split('/');
            for (index, value) in path.split('/').enumerate() {
                if index > 0 {
                    result.push('/');
                }
                if literals
                    .next()
                    .is_some_and(|literal| !literal.starts_with('{'))
                {
                    result.extend(
                        value
                            .chars()
                            .map(|character| character.to_ascii_lowercase()),
                    );
                } else {
                    result.push_str(value);
                }
            }
            return result;
        }
    }
    path.to_owned()
}

fn is_api(path: &str) -> bool {
    path.strip_prefix('/')
        .and_then(|path| path.split('/').next())
        .is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case("api") || prefix.eq_ignore_ascii_case("qbittorrent")
        })
}

fn has_prefix(path: &str, prefix: &str) -> bool {
    path.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        && (path.len() == prefix.len() || path.as_bytes().get(prefix.len()) == Some(&b'/'))
}

fn should_log(path: &str) -> bool {
    has_prefix(path, "/api/v2")
        || has_prefix(path, "/api/torrents")
        || has_prefix(path, "/qbittorrent/api/v2")
}

fn sensitive(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "password" | "passwd" | "secret" | "apikey" | "token"
    )
}

fn redact_pairs(value: &str) -> String {
    let mut output = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in url::form_urlencoded::parse(value.as_bytes()) {
        output.append_pair(&key, if sensitive(&key) { "*****" } else { &value });
    }
    output.finish()
}

fn redact_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                if sensitive(key) {
                    *value = "*****".into();
                } else {
                    redact_json(value);
                }
            }
        }
        serde_json::Value::Array(values) => values.iter_mut().for_each(redact_json),
        _ => {}
    }
}

async fn log_request(request: Request, logs: &LogControl) -> Result<Request, ApiError> {
    let (parts, body) = request.into_parts();
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let json = content_type
        .split(';')
        .next()
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"));
    let form = content_type.starts_with("application/x-www-form-urlencoded");
    let multipart = content_type.starts_with("multipart/form-data");
    let mut logged_body = String::new();
    let body = if json || form || multipart {
        let bytes = to_bytes(body, MAX_BODY_SIZE).await.map_err(|_| {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body too large or unreadable",
            )
        })?;
        if json {
            // Never echo malformed JSON: credentials may be present even when parsing fails.
            logged_body = serde_json::from_slice(&bytes).map_or_else(
                |_| "[invalid JSON]".to_owned(),
                |mut value| {
                    redact_json(&mut value);
                    value.to_string()
                },
            );
        } else if multipart {
            logged_body = log_multipart(&bytes, content_type).await;
        } else {
            logged_body = redact_pairs(&String::from_utf8_lossy(&bytes));
        }
        Body::from(bytes)
    } else {
        body
    };
    tracing::debug!(method = %parts.method, path = %logs.redact(parts.uri.path()), query = %logs.redact(&redact_pairs(parts.uri.query().unwrap_or(""))), body = %logs.redact(&logged_body), "HTTP request");
    Ok(Request::from_parts(parts, body))
}

async fn log_multipart(bytes: &axum::body::Bytes, content_type: &str) -> String {
    use axum::extract::{FromRequest, Multipart};
    let request = Request::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes.clone()))
        .expect("existing content type is a valid header");
    let Ok(mut multipart) = Multipart::from_request(request, &()).await else {
        return "[invalid multipart]".to_owned();
    };
    let mut fields = Vec::new();
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.file_name().is_some() {
                    continue;
                }
                let name = field.name().unwrap_or("").to_owned();
                let value = if sensitive(&name) {
                    "*****".to_owned()
                } else {
                    match field.text().await {
                        Ok(value) => value,
                        Err(_) => return "[invalid multipart]".to_owned(),
                    }
                };
                fields.push((name, value));
            }
            Ok(None) => {
                return url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(fields)
                    .finish();
            }
            Err(_) => return "[invalid multipart]".to_owned(),
        }
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "panic response callbacks receive an owned payload"
)]
fn panic_response(payload: Box<dyn Any + Send + 'static>) -> Response {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("Internal server error");
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
}

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web/build"]
#[allow_missing = true]
struct WebAssets;

async fn magnet_redirect(uri: Uri) -> Redirect {
    let query = uri
        .query()
        .map_or(String::new(), |query| format!("?{query}"));
    Redirect::temporary(&format!("./#/add{query}"))
}

async fn static_file(request: Request) -> Response {
    let path = request.uri().path();
    if is_api(path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    ) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let asset_path = path.trim_start_matches('/');
    let requested = if asset_path.is_empty() {
        "index.html"
    } else {
        asset_path
    };
    let asset = WebAssets::get(requested)
        .map(|asset| (requested, asset))
        .or_else(|| {
            if requested.contains('.') {
                None
            } else {
                WebAssets::get("index.html").map(|asset| ("index.html", asset))
            }
        });
    let mut response = if let Some((name, asset)) = asset {
        asset_response(name, asset.data)
    } else if requested == "index.html" {
        ([(header::CACHE_CONTROL, "no-cache")], "rd-client is running. The web UI is missing; build the web UI to use the browser application.\n").into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    };
    if request.method() == axum::http::Method::HEAD {
        *response.body_mut() = Body::empty();
    }
    response
}

fn asset_response(name: &str, bytes: std::borrow::Cow<'static, [u8]>) -> Response {
    let mime = mime_guess::from_path(name)
        .first_or_octet_stream()
        .to_string();
    let cache = if name == "index.html" {
        "no-cache"
    } else if name.starts_with("_app/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_str()),
            (header::CACHE_CONTROL, cache),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_have_mime_types_and_appropriate_cache_policies() {
        let index = asset_response("index.html", b"<html></html>".as_slice().into());
        assert_eq!(index.headers()[header::CONTENT_TYPE], "text/html");
        assert_eq!(index.headers()[header::CACHE_CONTROL], "no-cache");
        let script = asset_response("_app/immutable/start.js", b"code".as_slice().into());
        assert!(
            script.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .contains("javascript")
        );
        assert_eq!(
            script.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let css = asset_response("theme.css", b"style".as_slice().into());
        assert_eq!(css.headers()[header::CONTENT_TYPE], "text/css");
        assert_eq!(css.headers()[header::CACHE_CONTROL], "no-cache");
    }
}

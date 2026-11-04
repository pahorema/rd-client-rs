//! Router assembly, path handling, and plain-text API errors.

use std::{any::Any, fmt, sync::Arc};

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::{StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::MethodRouter,
};
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
    /// Start an application with no API endpoints.
    pub fn new(base_path: &str, logs: LogControl) -> Self {
        Self {
            router: Router::new().fallback(|| async { StatusCode::NOT_FOUND }),
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

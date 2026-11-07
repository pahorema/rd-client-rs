//! Persisted settings, provider profile and explicit diagnostics for the web UI.

use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    routing::{get, post},
};
use rdc_model::settings::SettingEntry;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    app::{AppState, CaseInsensitive, internal, required},
    http::ApiError,
};

/// All settings and diagnostic paths; mount this router behind authentication.
pub(crate) const PATHS: &[&str] = &[
    "/api/settings",
    "/api/settings/profile",
    "/api/settings/version",
    "/api/settings/testpath",
    "/api/settings/testdownloadspeed",
    "/api/settings/testwritespeed",
    "/api/settings/testaria2cconnection",
];

/// Settings, profile and diagnostic routes requiring the authentication policy.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(PATHS[0], get(list).put(update))
        .route(PATHS[1], get(profile))
        .route(PATHS[2], get(version))
        .route(PATHS[3], post(test_path))
        .route(PATHS[4], get(test_download_speed))
        .route(PATHS[5], get(test_write_speed))
        .route(PATHS[6], post(test_aria2c_connection))
}

#[derive(Deserialize)]
struct SettingUpdate {
    key: Option<String>,
    value: Option<Value>,
}

#[derive(Deserialize)]
struct PathRequest {
    path: Option<String>,
}

#[derive(Deserialize)]
struct Aria2Request {
    url: Option<String>,
    secret: Option<String>,
}

async fn list(State(state): State<AppState>) -> Json<Vec<SettingEntry>> {
    Json(state.engine.settings().entries().collect())
}

async fn update(
    State(state): State<AppState>,
    input: Result<Json<Option<Vec<CaseInsensitive<SettingUpdate>>>>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let values = required(input)?;
    let values = values
        .into_iter()
        .filter_map(|CaseInsensitive(SettingUpdate { key, value })| {
            key.map(|key| (key, stored_value(value)))
        })
        .collect();
    state
        .engine
        .update_settings(values)
        .await
        .map_err(internal)?;
    Ok(StatusCode::OK)
}

fn stored_value(value: Option<Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        Value::String(value) => Some(value),
        Value::Bool(value) => Some(if value { "True" } else { "False" }.to_owned()),
        other => Some(other.to_string()),
    }
}

async fn profile(State(state): State<AppState>) -> Result<Json<rdc_engine::Profile>, ApiError> {
    state.engine.profile().await.map(Json).map_err(internal)
}

async fn version() -> Json<Value> {
    Json(json!({"version": env!("CARGO_PKG_VERSION")}))
}

async fn test_path(
    State(state): State<AppState>,
    input: Result<Json<Option<CaseInsensitive<PathRequest>>>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let request = required(input)?.0;
    let path = request.path.as_deref().unwrap_or_default();
    if path.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    state
        .engine
        .test_path(path.trim_end_matches(['/', '\\']))
        .await
        .map_err(internal)?;
    Ok(StatusCode::OK)
}

async fn test_download_speed(State(state): State<AppState>) -> Result<Json<u64>, ApiError> {
    state
        .engine
        .test_download_speed()
        .await
        .map(Json)
        .map_err(internal)
}

async fn test_write_speed(State(state): State<AppState>) -> Result<Json<f64>, ApiError> {
    state
        .engine
        .test_write_speed()
        .await
        .map(Json)
        .map_err(internal)
}

async fn test_aria2c_connection(
    State(state): State<AppState>,
    input: Result<Json<Option<CaseInsensitive<Aria2Request>>>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let request = required(input)?.0;
    let url = request.url.as_deref().unwrap_or_default();
    if url.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Invalid Url"));
    }
    let version = state
        .engine
        .test_aria2(url, request.secret.as_deref())
        .await
        .map_err(internal)?;
    Ok(Json(
        json!({"version": version.version, "enabledFeatures": version.enabled_features}),
    ))
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Arc};

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, header},
        response::Response,
    };
    use rdc_engine::testing::TestEngine;
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::{
        app,
        auth::{AuthPolicy, Sessions},
        logging,
    };

    struct Fixture {
        test: TestEngine,
        router: Router,
        cookie: String,
    }

    impl Fixture {
        async fn new() -> Self {
            let test = TestEngine::new().await.expect("isolated engine starts");
            let sessions = Sessions::open(
                test.store.clone(),
                &test.tempdir.path().join("sessions.key"),
            )
            .await
            .expect("session key initializes");
            let policy = AuthPolicy::new(sessions.clone(), Arc::new(|| false));
            let (_, logs) = logging::subscriber(io::sink);
            let router = app::build(
                app::Backend {
                    engine: test.engine.clone(),
                    store: test.store.clone(),
                    sessions,
                    policy,
                },
                "",
                logs,
                tokio_util::sync::CancellationToken::new(),
            );
            let mut fixture = Self {
                test,
                router,
                cookie: String::new(),
            };
            let created = fixture
                .send(
                    "POST",
                    "/api/authentication/create",
                    Some(json!({"userName":"alice","password":"Passw0rd!"})),
                )
                .await;
            assert_eq!(created.status(), StatusCode::OK);
            fixture.cookie = created.headers()[header::SET_COOKIE]
                .to_str()
                .expect("session cookie")
                .split(';')
                .next()
                .expect("SID first")
                .to_owned();
            fixture
        }

        async fn send(&self, method: &str, path: &str, value: Option<Value>) -> Response {
            let mut request = Request::builder().method(method).uri(path);
            if value.is_some() {
                request = request.header(header::CONTENT_TYPE, "application/json");
            }
            if !self.cookie.is_empty() {
                request = request.header(header::COOKIE, self.cookie.as_str());
            }
            let body = value.map_or_else(Body::empty, |value| Body::from(value.to_string()));
            self.router
                .clone()
                .oneshot(request.body(body).expect("valid request"))
                .await
                .expect("router responds")
        }
    }

    async fn output(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body is readable");
        serde_json::from_slice(&bytes).expect("JSON response")
    }

    async fn text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body is readable");
        String::from_utf8(bytes.to_vec()).expect("UTF-8 response")
    }

    #[tokio::test]
    async fn settings_list_and_update_only_use_known_keys_and_values() {
        let fixture = Fixture::new().await;
        let anonymous = {
            let request = Request::builder()
                .uri("/api/settings")
                .body(Body::empty())
                .expect("valid request");
            fixture
                .router
                .clone()
                .oneshot(request)
                .await
                .expect("router responds")
        };
        assert_eq!(anonymous.status(), StatusCode::FORBIDDEN);
        let list = fixture.send("GET", "/Api/Settings", None).await;
        assert_eq!(list.status(), StatusCode::OK);
        let entries = output(list).await;
        let entries = entries.as_array().expect("catalogue array");
        assert!(
            entries
                .iter()
                .any(|entry| entry["key"] == "General:LogLevel"
                    && entry["type"] == "Enum"
                    && entry["enumValues"]["2"] == "Information")
        );
        assert_eq!(
            fixture
                .send("PUT", "/Api/Settings", Some(Value::Null))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        let malformed = Request::builder()
            .method("PUT")
            .uri("/Api/Settings")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, fixture.cookie.as_str())
            .body(Body::from("{"))
            .expect("valid request");
        let malformed = fixture
            .router
            .clone()
            .oneshot(malformed)
            .await
            .expect("router responds");
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        assert_eq!(text(malformed).await, "");
        let updated = fixture
            .send(
                "PUT",
                "/Api/Settings",
                Some(json!([
                    {"KEY":"General:LogLevel","VaLuE":2,"displayName":"ignored"},
                    {"key":"Provider:AutoImport","value":true},
                    {"Key":"Watch:Path","VALUE":"queue"},
                    {"key":"Unknown:Setting","value":"ignored"}
                ])),
            )
            .await;
        assert_eq!(updated.status(), StatusCode::OK);
        let settings = fixture
            .test
            .store
            .settings()
            .load()
            .await
            .expect("settings load");
        assert_eq!(settings.general.log_level.to_string(), "Information");
        assert!(settings.provider.auto_import);
        assert_eq!(settings.watch.path.as_deref(), Some("queue"));
        assert_eq!(
            fixture.test.engine.settings().watch.path.as_deref(),
            Some("queue")
        );
        assert_eq!(
            fixture
                .send(
                    "PUT",
                    "/Api/Settings",
                    Some(json!([
                        {"key":"Watch:Path","value":null},
                        {"key":"Provider:AutoImport","value":false}
                    ]))
                )
                .await
                .status(),
            StatusCode::OK
        );
        let settings = fixture
            .test
            .store
            .settings()
            .load()
            .await
            .expect("settings reload");
        assert_eq!(settings.watch.path, None);
        assert!(!settings.provider.auto_import);
        fixture.test.engine.shutdown().await;
    }

    #[tokio::test]
    async fn profile_version_and_path_diagnostics_report_results_and_errors() {
        let fixture = Fixture::new().await;
        let profile = fixture.send("GET", "/Api/Settings/Profile", None).await;
        assert_eq!(profile.status(), StatusCode::OK);
        let profile = output(profile).await;
        assert_eq!(profile["provider"], "RealDebrid");
        assert_eq!(profile["premium"], false);
        let version = output(fixture.send("GET", "/Api/Settings/Version", None).await).await;
        assert_eq!(version["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            fixture
                .send("POST", "/Api/Settings/TestPath", Some(Value::Null))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        let blank = fixture
            .send("POST", "/Api/Settings/TestPath", Some(json!({"path":""})))
            .await;
        assert_eq!(blank.status(), StatusCode::BAD_REQUEST);
        assert_eq!(text(blank).await, "Invalid path");
        let absent = fixture.test.tempdir.path().join("absent");
        let missing = fixture
            .send(
                "POST",
                "/Api/Settings/TestPath",
                Some(json!({"path":absent.to_string_lossy()})),
            )
            .await;
        assert_eq!(missing.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            text(missing).await,
            format!("Path {} does not exist", absent.display())
        );
        let downloads = fixture.test.tempdir.path().join("downloads");
        let valid = fixture
            .send(
                "POST",
                "/Api/Settings/TestPath",
                Some(json!({"path":format!( "{}/", downloads.display())})),
            )
            .await;
        assert_eq!(valid.status(), StatusCode::OK);
        assert!(
            std::fs::read_dir(&downloads)
                .expect("download dir")
                .next()
                .is_none(),
            "path probe must remove its file"
        );
        let speed = fixture
            .send("GET", "/Api/Settings/TestWriteSpeed", None)
            .await;
        assert_eq!(speed.status(), StatusCode::OK);
        assert!(
            output(speed)
                .await
                .as_f64()
                .is_some_and(|speed| speed > 0.0)
        );
        assert!(
            std::fs::read_dir(downloads)
                .expect("download dir")
                .next()
                .is_none(),
            "write probe must remove its file"
        );
        fixture.test.engine.shutdown().await;
    }

    #[tokio::test]
    async fn aria2_diagnostic_validates_input_and_returns_rpc_version() {
        use std::io::{Read, Write};

        let fixture = Fixture::new().await;
        let invalid = fixture
            .send(
                "POST",
                "/Api/Settings/TestAria2cConnection",
                Some(json!({"url":""})),
            )
            .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(text(invalid).await, "Invalid Url");
        let invalid = fixture
            .send(
                "POST",
                "/Api/Settings/TestAria2cConnection",
                Some(Value::Null),
            )
            .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let bad_url = fixture
            .send(
                "POST",
                "/Api/Settings/TestAria2cConnection",
                Some(json!({"URL":"http://127.0.0.1:9"})),
            )
            .await;
        assert_eq!(bad_url.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_ne!(text(bad_url).await, "");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback bind");
        let url = format!(
            "http://{}/jsonrpc",
            listener.local_addr().expect("loopback address")
        );
        let server = tokio::task::spawn_blocking(move || {
            let (mut connection, _) = listener.accept().expect("aria2 RPC arrives");
            let mut buffer = [0; 4096];
            let _ = connection.read(&mut buffer).expect("RPC request reads");
            let payload = r#"{"jsonrpc":"2.0","id":"rd-client","result":{"version":"1.36.0","enabledFeatures":["HTTPS"]}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            connection
                .write_all(response.as_bytes())
                .expect("RPC reply writes");
        });
        let result = fixture
            .send(
                "POST",
                "/Api/Settings/TestAria2cConnection",
                Some(json!({"url":url,"secret":"secret"})),
            )
            .await;
        assert_eq!(result.status(), StatusCode::OK);
        let result = output(result).await;
        assert_eq!(
            result,
            json!({"version":"1.36.0","enabledFeatures":["HTTPS"]})
        );
        server.await.expect("RPC server completes");
        fixture.test.engine.shutdown().await;
    }
}

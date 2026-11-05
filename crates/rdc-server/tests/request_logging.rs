use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
};
use rdc_model::LogLevel;
use rdc_server::{http::AppBuilder, logging};
use tower::ServiceExt;

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("capture lock poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn requests_redact_query_form_json_and_dynamic_secrets_without_changing_bodies() {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(buffer.clone());
    let (subscriber, logs) = logging::subscriber(move || writer.clone());
    logs.set_level(LogLevel::Debug).unwrap();
    logs.set_secrets(["provider+secret".to_owned()]);
    let _guard = tracing::subscriber::set_default(subscriber);
    let app = AppBuilder::new("/base", logs)
        .route(
            "/api/v2/auth/login",
            post(|body: String| async move { body }),
        )
        .route("/api/torrents", post(|body: String| async move { body }))
        .route(
            "/qbittorrent/api/v2/auth/login",
            get(|| async { StatusCode::UNAUTHORIZED }),
        )
        .build(());
    for (uri, content_type, body) in [
        (
            "/base/API/V2/AUTH/LOGIN?password=query-password&token=query-token&username=query-user&user_name=query-alias&name=visible",
            "application/x-www-form-urlencoded",
            "username=visible-user&user_name=form-alias&PaSsWoRd=form-password&passwd=form-passwd&secret=form-secret&apiKey=form-apikey&token=form-token&other=provider%2Bsecret",
        ),
        (
            "/api/torrents",
            "application/json",
            r#"{"PASSWORD":"json-password","nested":{"token":"json-token","safe":"provider+secret"},"list":[{"secret":"json-secret"}],"visible":"visible-json"}"#,
        ),
        (
            "/api/torrents",
            "application/json",
            r#"{"password":"malformed-secret""#,
        ),
        (
            "/api/torrents",
            "multipart/form-data; boundary=testboundary",
            "--testboundary\r\nContent-Disposition: form-data; name=\"Password\"\r\n\r\nmultipart-password\r\n--testboundary\r\nContent-Disposition: form-data; name=\"visible\"\r\n\r\nvisible-multipart\r\n--testboundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"private-file\"\r\n\r\nprivate-file-data\r\n--testboundary--\r\n",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post(uri)
                    .header("content-type", content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let echoed = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(echoed.as_ref(), body.as_bytes());
    }
    app.oneshot(
        Request::get("/qbittorrent/api/v2/auth/login?password=alias-password")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    for secret in [
        "query-password",
        "query-user",
        "query-alias",
        "visible-user",
        "form-alias",
        "query-token",
        "form-password",
        "form-passwd",
        "form-secret",
        "form-apikey",
        "form-token",
        "provider+secret",
        "provider%2Bsecret",
        "json-password",
        "json-token",
        "json-secret",
        "malformed-secret",
        "alias-password",
        "multipart-password",
        "private-file-data",
    ] {
        assert!(!output.contains(secret), "leaked {secret}: {output}");
    }
    for visible in [
        "visible-json",
        "visible-multipart",
        "visible",
        "POST",
        "HTTP response",
        "403",
        "*****",
    ] {
        assert!(output.contains(visible), "missing {visible}: {output}");
    }
}

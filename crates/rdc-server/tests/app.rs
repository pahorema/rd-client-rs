use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use rdc_engine::testing::TestEngine;
use rdc_server::{
    app,
    auth::{AuthPolicy, Sessions},
    logging,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[tokio::test]
async fn assembled_app_serves_the_torrent_client_api_under_the_base_path() {
    let fixture = TestEngine::new().await.expect("test engine");
    let sessions = Sessions::open(
        fixture.store.clone(),
        &fixture.tempdir.path().join("session.key"),
    )
    .await
    .expect("sessions");
    let policy = AuthPolicy::new(sessions.clone(), Arc::new(|| true));
    let (_, logs) = logging::subscriber(std::io::sink);
    let router = app::build(
        app::Backend {
            engine: fixture.engine.clone(),
            store: fixture.store.clone(),
            sessions,
            policy,
        },
        "/client",
        logs,
        CancellationToken::new(),
    );
    for path in ["/client/api/v2/app/version", "/api/v2/app/version"] {
        let response = router
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = to_bytes(response.into_body(), 1024).await.expect("body");
        assert!(body.starts_with(b"v"), "{path}: {body:?}");
    }
}

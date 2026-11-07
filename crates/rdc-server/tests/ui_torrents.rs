use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use rdc_engine::{TorrentTemplate, testing::TestEngine};
use rdc_model::AuthenticationType;
use rdc_server::{
    app,
    auth::{AuthPolicy, Sessions},
    logging,
};
use serde_json::{Value, json};
use tower::ServiceExt;

const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Movie";

/// An isolated engine with authentication disabled, served under `base`.
async fn fixture(base: &str) -> (TestEngine, Router) {
    let fixture = TestEngine::new().await.expect("test engine");
    fixture
        .engine
        .update_setting("General:AuthenticationType", Some("1"))
        .await
        .expect("disable auth");
    assert_eq!(
        fixture.engine.settings().general.authentication_type,
        AuthenticationType::None
    );
    let sessions = Sessions::open(
        fixture.store.clone(),
        &fixture.tempdir.path().join("session.key"),
    )
    .await
    .expect("sessions");
    let engine = fixture.engine.clone();
    let policy = AuthPolicy::new(
        sessions.clone(),
        Arc::new(move || engine.settings().general.authentication_type == AuthenticationType::None),
    );
    let (_, logs) = logging::subscriber(std::io::sink);
    let router = app::build(
        app::Backend {
            engine: fixture.engine.clone(),
            store: fixture.store.clone(),
            sessions,
            policy,
        },
        base,
        logs,
    );
    (fixture, router)
}

fn request(method: &str, path: &str, body: Body, content_type: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    builder.body(body).expect("request")
}

async fn response(router: &Router, method: &str, path: &str, json: Value) -> (StatusCode, String) {
    let request = request(
        method,
        path,
        Body::from(json.to_string()),
        Some("application/json"),
    );
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("HTTP response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000)
        .await
        .expect("body");
    (status, String::from_utf8(bytes.to_vec()).expect("UTF-8"))
}

#[tokio::test]
async fn torrent_queries_return_list_detail_and_status_shapes() {
    let (fixture, router) = fixture("/movies").await;
    let template = TorrentTemplate {
        category: Some("movies".to_owned()),
        ..TorrentTemplate::default()
    };
    let torrent = fixture
        .engine
        .add_magnet(MAGNET, template)
        .await
        .expect("seed torrent");

    let (status, body) = response(&router, "GET", "/movies/aPi/ToRrEnTs", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let rows: Value = serde_json::from_str(&body).expect("list JSON");
    let id = torrent.torrent_id.to_string();
    assert_eq!(rows[0]["torrentId"], id.as_str());
    assert_eq!(rows[0]["category"], "movies");
    assert_eq!(rows[0]["files"], json!([]));
    assert_eq!(rows[0]["downloads"], json!([]));
    assert_eq!(rows[0]["fileOrMagnet"], Value::Null);
    assert_eq!(rows[0]["statusText"], "Not Yet Added to Provider");
    assert!(rows[0].get("type").is_none());

    let (status, body) = response(
        &router,
        "GET",
        &format!("/movies/Api/Torrents/Get/{id}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let detail: Value = serde_json::from_str(&body).expect("detail JSON");
    assert!(
        detail["fileOrMagnet"]
            .as_str()
            .expect("magnet payload")
            .starts_with("magnet:")
    );

    for missing in ["00000000-0000-0000-0000-000000000000", "invalid"] {
        let (status, body) = response(
            &router,
            "GET",
            &format!("/movies/Api/Torrents/Get/{missing}"),
            json!({}),
        )
        .await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::NOT_FOUND, ""),
            "{missing}"
        );
    }

    let (status, body) = response(
        &router,
        "GET",
        "/movies/Api/Torrents/RateLimitStatus",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("rate limit")["secondsRemaining"],
        0.0
    );
    let (status, body) = response(
        &router,
        "GET",
        "/movies/Api/Torrents/DiskSpaceStatus",
        json!({}),
    )
    .await;
    assert_eq!((status, body.as_str()), (StatusCode::NO_CONTENT, ""));
}

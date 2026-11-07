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
const TORRENT: &[u8] =
    b"d4:infod6:lengthi1e4:name5:Movie12:piece lengthi1e6:pieces20:abcdefghijklmnopqrstee";

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

#[tokio::test]
async fn torrent_changes_cover_add_update_retry_delete_and_errors() {
    let (fixture, router) = fixture("/movies").await;
    let (status, error) = response(
        &router,
        "POST",
        "/movies/Api/Torrents/UploadMagnet",
        json!({"magnetLink": ""}),
    )
    .await;
    assert_eq!(
        (status, error.as_str()),
        (StatusCode::BAD_REQUEST, "Invalid magnet link")
    );
    let (status, error) = response(
        &router,
        "POST",
        "/movies/Api/Torrents/UploadMagnet",
        json!({"magnetLink": MAGNET, "torrent": null}),
    )
    .await;
    assert_eq!(
        (status, error.as_str()),
        (StatusCode::BAD_REQUEST, "Invalid Torrent")
    );
    let (status, body) = response(
        &router,
        "POST",
        "/movies/Api/Torrents/UploadMagnet",
        json!({"magnetLink": "not-a-magnet", "torrent": {}}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("not-a-magnet"), "{body}");

    let (status, _) = response(
        &router,
        "POST",
        "/movies/Api/Torrents/UploadMagnet",
        json!({"magnetLink": MAGNET, "torrent": {"category": "movies", "priority": 4}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = fixture.engine.torrents().await.expect("added torrent");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].category.as_deref(), Some("movies"));
    assert_eq!(rows[0].priority, Some(4));
    let id = rows[0].torrent_id;

    let (status, _) = response(
        &router,
        "PUT",
        "/movies/Api/Torrents/Update",
        json!({
            "torrentId": id,
            "category": "series",
            "priority": 2,
            "downloadRetryAttempts": 3,
            "hash": "ignored",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = fixture.engine.torrents().await.expect("updated torrent");
    assert_eq!(rows[0].category.as_deref(), Some("series"));
    assert_eq!(rows[0].download_retry_attempts, 3);

    let (status, _) = response(
        &router,
        "POST",
        "/movies/Api/Torrents/RetryDownload/00000000-0000-0000-0000-000000000000",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = response(
        &router,
        "POST",
        &format!("/movies/Api/Torrents/Delete/{id}"),
        json!({"deleteData": true, "deleteRdTorrent": false, "deleteLocalFiles": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let remaining = fixture.engine.torrents().await.expect("remaining torrents");
    assert_eq!(remaining.len(), 0);
}

#[tokio::test]
async fn torrent_file_uploads_and_availability_checks() {
    let (fixture, router) = fixture("").await;
    let boundary = "rdc-boundary";
    let mut payload = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"formData\"\r\n\r\n\
         {{\"torrent\":{{\"priority\":3}}}}\r\n--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"sample.torrent\"\r\n\
         Content-Type: application/x-bittorrent\r\n\r\n"
    )
    .into_bytes();
    payload.extend_from_slice(TORRENT);
    payload.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let upload = router
        .clone()
        .oneshot(request(
            "POST",
            "/Api/Torrents/UploadFile",
            Body::from(payload),
            Some(&format!("multipart/form-data; boundary={boundary}")),
        ))
        .await
        .expect("upload response");
    assert_eq!(upload.status(), StatusCode::OK);
    let rows = fixture.engine.torrents().await.expect("file list");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].is_file);
    assert_eq!(rows[0].priority, Some(3));

    let (status, body) = response(
        &router,
        "POST",
        "/Api/Torrents/CheckFilesMagnet",
        json!({"magnetLink": MAGNET}),
    )
    .await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "[]"));
    let (status, body) = response(
        &router,
        "POST",
        "/Api/Torrents/VerifyRegex",
        json!({"magnetLink": MAGNET, "includeRegex": "["}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let result: Value = serde_json::from_str(&body).expect("regex response");
    assert_ne!(result["includeError"].as_str().expect("error"), "");
    assert_eq!(result["selectedFiles"], json!([]));
}

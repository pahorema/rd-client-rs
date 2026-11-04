//! HIGHWAY client against a mock API.

use std::sync::Arc;
use std::time::Duration;

use rdc_debrid::{
    DebridError, Highway, ProviderConfig, ProviderHttp, RateLimitCoordinator, RetryPolicy,
    is_spool_id,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ENDPOINT: &str = "/center-api/transfers.php";
const SPOOL_ID: &str = "3f9a1c2b4d5e6f708192a3b4c5d6e7f8";

fn config(api_key: &str) -> ProviderConfig {
    ProviderConfig {
        api_key: api_key.to_owned(),
        api_hostname: None,
        timeout: Duration::from_secs(5),
        prefer_zipped_downloads: false,
    }
}

fn client_with_key(server: &MockServer, api_key: &str) -> Highway {
    let http = ProviderHttp::new(
        Duration::from_secs(5),
        Arc::new(RateLimitCoordinator::new()),
    )
    .expect("client builds")
    .with_retry_policy(RetryPolicy {
        max_retries: 0,
        base_delay: Duration::from_millis(1),
    });
    Highway::new(&config(api_key), http).with_endpoint(&format!("{}{ENDPOINT}", server.uri()))
}

fn client(server: &MockServer) -> Highway {
    client_with_key(server, "test-token")
}

fn json_response(status: u16, body: &Value) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(body)
}

fn transfer(id: &str, state: &str) -> Value {
    json!({
        "id": id,
        "service": "torrent",
        "name": "Some.Release",
        "state": state,
        "state_detail": "boom",
        "progress": 62,
        "size": 1000,
        "downloaded": 400,
        "eta": 6,
        "seeder": 9,
        "content_path": "TORRENT/Some.Release",
        "download_url": "https://nl.dwld.link/dav/TORRENT/Some.Release/?download=zip",
        "files": [
            { "path": "Some.Release/Some.Release.mkv", "size": 990, "partial": false,
              "download_url": "https://nl.dwld.link/dav/TORRENT/Some.Release/Some.Release.mkv?sig=1" },
            { "path": "Some.Release/Some.Release.part", "size": 10, "partial": true, "download_url": null }
        ]
    })
}

async fn mount_listing(server: &MockServer, transfers: &Value) {
    Mock::given(method("GET"))
        .and(path(ENDPOINT))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(json_response(200, &json!({ "transfers": transfers })))
        .mount(server)
        .await;
}

#[test]
fn spool_ids_are_32_hex_characters() {
    assert!(is_spool_id(SPOOL_ID));
    assert!(!is_spool_id("4213"));
    assert!(!is_spool_id("3f9a1c2b4d5e6f708192a3b4c5d6e7fz"));
    assert!(!is_spool_id("3f9a1c2b4d5e6f708192a3b4c5d6e7f80"));
}

#[tokio::test]
async fn listing_returns_torrent_transfers_and_skips_other_services() {
    let server = MockServer::start().await;
    let usenet =
        json!({ "id": SPOOL_ID, "service": "usenet", "name": "Usenet.Release", "state": "queued" });
    let numeric = json!({ "id": 4214, "service": "torrent", "name": "Other", "state": "queued" });
    mount_listing(
        &server,
        &json!([transfer("4213", "downloading"), usenet, numeric]),
    )
    .await;

    let torrents = client(&server).list_torrents().await.expect("list");

    let ids: Vec<&str> = torrents.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["4213", "4214"]);
}

#[tokio::test]
async fn listing_maps_transfer_fields() {
    let server = MockServer::start().await;
    mount_listing(&server, &json!([transfer("4213", "completed")])).await;

    let torrents = client(&server).list_torrents().await.expect("list");

    let torrent = &torrents[0];
    assert_eq!(torrent.hash, "4213");
    assert_eq!(torrent.filename, "Some.Release");
    assert_eq!(torrent.original_filename.as_deref(), Some("Some.Release"));
    assert_eq!((torrent.bytes, torrent.original_bytes), (1000, 1000));
    assert_eq!(torrent.host.as_deref(), Some("TORRENT/Some.Release"));
    assert_eq!(torrent.progress, 62);
    assert_eq!(torrent.status.as_deref(), Some("completed"));
    assert_eq!(torrent.message.as_deref(), Some("boom"));
    assert!(torrent.added.is_some());
    assert!(torrent.ended.is_some(), "completed transfers have ended");
    assert_eq!(torrent.speed, Some(100), "600 bytes left in 6 seconds");
    assert_eq!(torrent.seeders, Some(9));
    let files = torrent.files.as_deref().expect("files");
    assert_eq!(files.len(), 2);
    assert_eq!((files[0].id, files[0].selected), (0, true));
    assert_eq!(
        (files[1].id, files[1].selected),
        (1, false),
        "partial files are not selected"
    );
    assert_eq!(files[1].path, "Some.Release/Some.Release.part");
}

#[tokio::test]
async fn listing_has_no_speed_or_end_while_the_eta_is_unknown() {
    let server = MockServer::start().await;
    let mut queued = transfer("4213", "queued");
    queued["eta"] = Value::Null;
    mount_listing(&server, &json!([queued])).await;

    let torrents = client(&server).list_torrents().await.expect("list");

    assert_eq!(torrents[0].speed, None);
    assert_eq!(torrents[0].ended, None);
}

#[tokio::test]
async fn missing_endpoint_answer_is_an_empty_listing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let torrents = client(&server).list_torrents().await.expect("list");

    assert_eq!(torrents.len(), 0);
}

#[tokio::test]
async fn blank_token_is_not_configured_and_sends_nothing() {
    let server = MockServer::start().await;

    let error = client_with_key(&server, "")
        .list_torrents()
        .await
        .expect_err("no token");

    assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
    assert_eq!(server.received_requests().await.expect("recorded").len(), 0);
}

async fn listing_error(response: ResponseTemplate) -> DebridError {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(response)
        .mount(&server)
        .await;
    client(&server).list_torrents().await.expect_err("failure")
}

#[tokio::test]
async fn invalid_tokens_report_an_authentication_failure() {
    for code in ["invalid_token", "authentication_required"] {
        let body = json!({ "error": code, "message": "Token rejected." });
        let error = listing_error(json_response(401, &body)).await;

        assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
        assert_eq!(
            error.to_string(),
            format!(
                "HIGHWAY authentication failed ({code}). Check the API token in the provider settings. Token rejected."
            )
        );
    }
}

#[tokio::test]
async fn foreign_tokens_report_the_wrong_account() {
    let body = json!({ "error": "system_feature_unavailable" });
    let error = listing_error(json_response(403, &body)).await;

    assert_eq!(
        error.to_string(),
        "The HIGHWAY token does not belong to a high-way.me account (system_feature_unavailable)."
    );
}

#[tokio::test]
async fn other_failures_report_status_code_and_message() {
    let body = json!({ "error": "internal", "message": "Database down" });
    let error = listing_error(json_response(500, &body)).await;

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "HIGHWAY request failed (500 internal): Database down"
    );
}

#[tokio::test]
async fn failures_without_error_body_report_the_raw_body() {
    let error = listing_error(ResponseTemplate::new(502).set_body_string("Bad gateway")).await;

    assert_eq!(
        error.to_string(),
        "HIGHWAY request failed (502): Bad gateway"
    );
}

#[tokio::test]
async fn too_many_requests_is_a_rate_limit() {
    let error = listing_error(ResponseTemplate::new(429)).await;

    assert_eq!(error.retry_after(), Some(Duration::from_secs(120)));
}

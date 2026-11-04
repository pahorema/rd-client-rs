//! AllDebrid client against a mocked API.

use std::sync::Arc;
use std::time::Duration;

use rdc_debrid::{
    AllDebrid, DebridError, ProviderConfig, ProviderHttp, RateLimitCoordinator, RetryPolicy,
};
use rdc_model::Torrent;
use serde_json::json;
use wiremock::matchers::{bearer_token, body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(api_key: &str, timeout: Duration) -> ProviderConfig {
    ProviderConfig {
        api_key: api_key.to_owned(),
        api_hostname: None,
        timeout,
        prefer_zipped_downloads: false,
    }
}

fn client_with(server: &MockServer, config: &ProviderConfig) -> AllDebrid {
    let http = ProviderHttp::new(
        Duration::from_secs(30),
        Arc::new(RateLimitCoordinator::new()),
    )
    .expect("client builds")
    .with_retry_policy(RetryPolicy {
        max_retries: 1,
        base_delay: Duration::from_millis(1),
    });
    AllDebrid::with_base_url(config, http, &format!("{}/v4.1/", server.uri()))
}

fn client(server: &MockServer) -> AllDebrid {
    client_with(server, &config("secret", Duration::from_secs(5)))
}

fn success(data: serde_json::Value) -> ResponseTemplate {
    let mut body = json!({"status": "success"});
    body["data"] = data;
    ResponseTemplate::new(200).set_body_json(body)
}

fn failure(code: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(json!({"status": "error", "error": {"code": code, "message": message}}))
}

fn torrent_with_id(id: &str) -> Torrent {
    Torrent {
        rd_id: Some(id.to_owned()),
        ..Torrent::default()
    }
}

async fn user_error(server: &MockServer, response: ResponseTemplate) -> DebridError {
    Mock::given(method("GET"))
        .and(path("/v4.1/user"))
        .respond_with(response)
        .mount(server)
        .await;
    client(server).user().await.expect_err("request fails")
}

#[tokio::test]
async fn user_is_requested_with_agent_and_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v4.1/user"))
        .and(query_param("agent", "rd-client"))
        .and(bearer_token("secret"))
        .respond_with(success(
            json!({"user": {"username": "alice", "isPremium": true, "premiumUntil": "1893456000"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("alice"));
    assert_eq!(
        user.expiration,
        Some("2030-01-01T00:00:00Z".parse().expect("timestamp"))
    );
}

#[tokio::test]
async fn free_users_have_no_expiration() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v4.1/user"))
        .respond_with(success(
            json!({"user": {"username": "bob", "premiumUntil": 0}}),
        ))
        .mount(&server)
        .await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.expiration, None);
}

#[tokio::test]
async fn blank_api_key_fails_without_sending_a_request() {
    let server = MockServer::start().await;
    let client = client_with(&server, &config("", Duration::from_secs(5)));

    let error = client.user().await.expect_err("no key");

    assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "The AllDebrid API key is not set in the settings"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn error_envelopes_report_message_and_code() {
    let server = MockServer::start().await;

    let error = user_error(
        &server,
        failure("AUTH_BAD_APIKEY", "The auth apikey is invalid"),
    )
    .await;

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "The auth apikey is invalid (AUTH_BAD_APIKEY)"
    );
}

#[tokio::test]
async fn invalid_magnet_id_errors_are_not_found() {
    let server = MockServer::start().await;

    let error = user_error(&server, failure("MAGNET_INVALID_ID", "Magnet not found")).await;

    assert!(error.is_not_found(), "{error:?}");
}

#[tokio::test]
async fn error_envelopes_without_message_are_unknown_errors() {
    let server = MockServer::start().await;
    let response = ResponseTemplate::new(200).set_body_string(r#"{"status":"error"}"#);

    let error = user_error(&server, response).await;

    assert_eq!(
        error.to_string(),
        r#"Unknown error. Response was: {"status":"error"}"#
    );
}

#[tokio::test]
async fn answers_that_are_not_json_are_reported() {
    let server = MockServer::start().await;

    let error = user_error(
        &server,
        ResponseTemplate::new(503).set_body_string("<html>busy</html>"),
    )
    .await;

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert!(error.to_string().contains("<html>busy</html>"), "{error}");
}

#[tokio::test]
async fn requests_time_out_after_the_configured_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v4.1/user"))
        .respond_with(success(json!({})).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let client = client_with(&server, &config("secret", Duration::from_millis(100)));

    let error = client.user().await.expect_err("times out");

    assert!(matches!(error, DebridError::Transport(_)), "{error:?}");
}

#[tokio::test]
async fn unrestricting_unlocks_the_link() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v4.1/link/unlock"))
        .and(query_param("agent", "rd-client"))
        .and(query_param("link", "https://alldebrid.com/f/abc"))
        .respond_with(success(
            json!({"link": "https://cdn.alldebrid.com/dl/abc/file.mkv", "filename": "file.mkv"}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let url = client(&server)
        .unrestrict("https://alldebrid.com/f/abc")
        .await
        .expect("unlocked");

    assert_eq!(url, "https://cdn.alldebrid.com/dl/abc/file.mkv");
}

#[tokio::test]
async fn unrestricting_without_link_fails() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v4.1/link/unlock"))
        .respond_with(success(json!({"delayed": 12})))
        .mount(&server)
        .await;

    let error = client(&server)
        .unrestrict("https://alldebrid.com/f/abc")
        .await
        .expect_err("no link");

    assert!(
        matches!(error, DebridError::InvalidResponse(_)),
        "{error:?}"
    );
}

#[tokio::test]
async fn deleting_a_torrent_deletes_its_magnet() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v4.1/magnet/delete"))
        .and(query_param("id", "42"))
        .respond_with(success(
            json!({"message": "Magnet was successfully deleted"}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    client(&server)
        .delete(&torrent_with_id("42"))
        .await
        .expect("deleted");
}

#[tokio::test]
async fn deleting_an_unknown_magnet_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v4.1/magnet/delete"))
        .respond_with(failure("MAGNET_INVALID_ID", "Magnet ID is invalid"))
        .mount(&server)
        .await;

    let error = client(&server)
        .delete(&torrent_with_id("42"))
        .await
        .expect_err("unknown");

    assert!(error.is_not_found(), "{error:?}");
    assert_eq!(
        error.to_string(),
        "Magnet ID is invalid (MAGNET_INVALID_ID)"
    );
}

/// Answers live-mode status calls sent with `counter`.
async fn mock_live(server: &MockServer, counter: u64, data: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/v4.1/magnet/status"))
        .and(query_param("agent", "rd-client"))
        .and(body_string_contains("session="))
        .and(body_string_contains(format!("counter={counter}")))
        .respond_with(success(data))
        .expect(1)
        .mount(server)
        .await;
}

fn magnet(id: i64, status_code: i64) -> serde_json::Value {
    json!({"id": id, "filename": format!("magnet-{id}"), "hash": format!("hash{id}"),
           "size": 400, "status": "Downloading", "statusCode": status_code,
           "downloaded": 100, "downloadSpeed": 2048, "seeders": 3,
           "uploadDate": 1_700_000_000, "completionDate": 0})
}

fn ids(torrents: &[rdc_debrid::RemoteTorrent]) -> Vec<&str> {
    torrents.iter().map(|torrent| torrent.id.as_str()).collect()
}

#[tokio::test]
async fn listing_a_full_sync_without_magnets_is_empty() {
    let server = MockServer::start().await;
    mock_live(
        &server,
        0,
        json!({"counter": 7, "fullsync": true, "magnets": []}),
    )
    .await;

    let torrents = client(&server).list_torrents().await.expect("listing");

    assert_eq!(torrents, Vec::new());
}

#[tokio::test]
async fn listing_an_empty_change_set_after_an_empty_full_sync_is_empty() {
    let server = MockServer::start().await;
    mock_live(
        &server,
        0,
        json!({"counter": 7, "fullsync": true, "magnets": []}),
    )
    .await;
    mock_live(&server, 7, json!({"counter": 8, "magnets": []})).await;
    let client = client(&server);

    client.list_torrents().await.expect("full sync");
    let torrents = client.list_torrents().await.expect("changes");

    assert_eq!(torrents, Vec::new());
}

#[tokio::test]
async fn a_new_full_sync_replaces_the_known_magnets() {
    let server = MockServer::start().await;
    mock_live(
        &server,
        0,
        json!({"counter": 7, "fullsync": true, "magnets": [magnet(1, 1), magnet(2, 1)]}),
    )
    .await;
    mock_live(
        &server,
        7,
        json!({"counter": 1, "fullsync": true, "magnets": [magnet(3, 4)]}),
    )
    .await;
    let client = client(&server);

    client.list_torrents().await.expect("full sync");
    let torrents = client.list_torrents().await.expect("second full sync");

    assert_eq!(ids(&torrents), ["3"]);
}

#[tokio::test]
async fn changes_with_a_new_magnet_add_it_to_the_known_ones() {
    let server = MockServer::start().await;
    mock_live(
        &server,
        0,
        json!({"counter": 7, "fullsync": true, "magnets": [magnet(1, 1), magnet(2, 1)]}),
    )
    .await;
    mock_live(&server, 7, json!({"counter": 8, "magnets": [magnet(3, 0)]})).await;
    let client = client(&server);

    client.list_torrents().await.expect("full sync");
    let torrents = client.list_torrents().await.expect("changes");

    assert_eq!(ids(&torrents), ["1", "2", "3"]);
}

#[tokio::test]
async fn changes_of_a_known_magnet_update_only_the_reported_properties() {
    let server = MockServer::start().await;
    mock_live(
        &server,
        0,
        json!({"counter": 7, "fullsync": true, "magnets": [magnet(1, 1), magnet(2, 1)]}),
    )
    .await;
    mock_live(
        &server,
        7,
        json!({"counter": 8, "magnets": [{"id": 2, "downloaded": 300, "downloadSpeed": 4096}]}),
    )
    .await;
    let client = client(&server);

    client.list_torrents().await.expect("full sync");
    let torrents = client.list_torrents().await.expect("changes");

    assert_eq!(ids(&torrents), ["1", "2"]);
    let changed = &torrents[1];
    assert_eq!(changed.progress, 75);
    assert_eq!(changed.speed, Some(4096));
    assert_eq!(changed.filename, "magnet-2");
    assert_eq!(changed.status.as_deref(), Some("Downloading"));
    assert_eq!(torrents[0].progress, 25);
}

#[tokio::test]
async fn listed_magnets_map_to_torrents() {
    let server = MockServer::start().await;
    mock_live(
        &server,
        0,
        json!({"counter": 1, "fullsync": true, "magnets": [
            {"id": 9, "filename": "Ready", "hash": "abc", "size": "100", "status": "Ready",
             "statusCode": 4, "uploadDate": 1_700_000_000, "completionDate": 1_700_000_600},
            {"id": 10, "filename": "Half", "size": 100, "downloaded": 50, "status": "Downloading",
             "statusCode": 1}
        ]}),
    )
    .await;

    let torrents = client(&server).list_torrents().await.expect("listing");

    let ready = &torrents[0];
    assert_eq!(ready.id, "9");
    assert_eq!(ready.filename, "Ready");
    assert_eq!(ready.original_filename.as_deref(), Some("Ready"));
    assert_eq!((ready.bytes, ready.original_bytes), (100, 100));
    assert_eq!((ready.status_code, ready.progress), (4, 0));
    assert_eq!((ready.host.as_deref(), ready.split), (None, 0));
    assert_eq!(
        ready.added,
        Some("2023-11-14T22:13:20Z".parse().expect("timestamp"))
    );
    assert_eq!(
        ready.ended,
        Some("2023-11-14T22:23:20Z".parse().expect("timestamp"))
    );
    assert_eq!(ready.links, Some(Vec::new()));
    assert_eq!(ready.files, None);
    let half = &torrents[1];
    assert_eq!(half.progress, 50);
    assert_eq!(half.ended, Some(jiff::Timestamp::UNIX_EPOCH));
}

#[tokio::test]
async fn magnets_without_status_are_not_listed() {
    let server = MockServer::start().await;
    mock_live(
        &server,
        0,
        json!({"counter": 1, "fullsync": true, "magnets": [{"id": 1, "filename": "x"}, magnet(2, 1)]}),
    )
    .await;

    let torrents = client(&server).list_torrents().await.expect("listing");

    assert_eq!(ids(&torrents), ["2"]);
}

#[tokio::test]
async fn adding_a_magnet_returns_the_magnet_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v4.1/magnet/upload"))
        .and(body_string_contains("magnets%5B%5D=magnet%3A%3Fxt"))
        .respond_with(success(
            json!({"magnets": [{"magnet": "magnet:?xt", "hash": "abc", "id": 123, "ready": true}]}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let id = client(&server)
        .add_magnet("magnet:?xt")
        .await
        .expect("added");

    assert_eq!(id, "123");
}

#[tokio::test]
async fn an_added_torrent_file_without_id_is_found_by_hash() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v4.1/magnet/upload/file"))
        .and(body_string_contains("filename=\"1.torrent\""))
        .respond_with(success(
            json!({"files": [{"file": "1.torrent", "hash": "ABC"}]}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v4.1/magnet/status"))
        .respond_with(success(json!({"magnets": [
            {"id": 1, "hash": "other", "status": "Ready"},
            {"id": 2, "hash": "abc", "status": "In Queue"}
        ]})))
        .expect(1)
        .mount(&server)
        .await;

    let id = client(&server)
        .add_torrent_file(b"d4:infod4:name1:aee")
        .await
        .expect("added");

    assert_eq!(id, "2");
}

#[tokio::test]
async fn rejected_magnets_report_their_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v4.1/magnet/upload"))
        .respond_with(success(json!({"magnets": [{"magnet": "x", "error":
            {"code": "MAGNET_INVALID_URI", "message": "Magnet is not valid"}}]})))
        .mount(&server)
        .await;

    let error = client(&server).add_magnet("x").await.expect_err("rejected");

    assert_eq!(
        error.to_string(),
        "Magnet is not valid (MAGNET_INVALID_URI)"
    );
}

#[tokio::test]
async fn rate_limit_texts_on_add_are_two_minute_rate_limits() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v4.1/magnet/upload"))
        .respond_with(failure("slow_down", "Too many requests"))
        .mount(&server)
        .await;

    let error = client(&server).add_magnet("x").await.expect_err("limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(120)));
}

//! Real-Debrid client against a mocked REST API.

use std::sync::Arc;
use std::time::Duration;

use rdc_debrid::{
    DebridError, ProviderConfig, ProviderHttp, RateLimitCoordinator, RealDebrid, RetryPolicy,
};
use serde_json::json;
use wiremock::matchers::{bearer_token, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(api_key: &str, timeout: Duration) -> ProviderConfig {
    ProviderConfig {
        api_key: api_key.to_owned(),
        api_hostname: None,
        timeout,
        prefer_zipped_downloads: false,
    }
}

fn client_with(server: &MockServer, config: &ProviderConfig) -> RealDebrid {
    let http = ProviderHttp::new(
        Duration::from_secs(30),
        Arc::new(RateLimitCoordinator::new()),
    )
    .expect("client builds")
    .with_retry_policy(RetryPolicy {
        max_retries: 1,
        base_delay: Duration::from_millis(1),
    });
    RealDebrid::with_base_url(config, http, &format!("{}/rest/1.0/", server.uri()))
}

fn client(server: &MockServer) -> RealDebrid {
    client_with(server, &config("secret", Duration::from_secs(5)))
}

async fn mock_user(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/rest/1.0/user"))
        .and(bearer_token("secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(server)
        .await;
}

async fn user_error(server: &MockServer, response: ResponseTemplate) -> DebridError {
    Mock::given(method("GET"))
        .and(path("/rest/1.0/user"))
        .respond_with(response)
        .mount(server)
        .await;
    client(server).user().await.expect_err("request fails")
}

#[tokio::test]
async fn user_reports_the_expiration_while_premium_time_is_left() {
    let server = MockServer::start().await;
    mock_user(
        &server,
        json!({"id": 1, "username": "alice", "type": "premium", "premium": 3600,
               "expiration": "2030-01-02T03:04:05.000Z"}),
    )
    .await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("alice"));
    assert_eq!(
        user.expiration,
        Some("2030-01-02T03:04:05Z".parse().expect("timestamp"))
    );
    assert!(!user.premium);
}

#[tokio::test]
async fn user_without_premium_time_has_no_expiration() {
    let server = MockServer::start().await;
    mock_user(
        &server,
        json!({"username": "bob", "type": "free", "premium": 0,
               "expiration": "2020-01-02T03:04:05.000Z"}),
    )
    .await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.expiration, None);
}

#[tokio::test]
async fn blank_api_key_fails_without_sending_a_request() {
    let server = MockServer::start().await;
    let client = client_with(&server, &config(" ", Duration::from_secs(5)));

    let error = client.user().await.expect_err("no key");

    assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "The Real-Debrid API key is not set in the settings"
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
async fn unknown_resource_error_code_is_not_found() {
    let server = MockServer::start().await;
    let response = ResponseTemplate::new(404)
        .set_body_json(json!({"error": "unknown_ressource", "error_code": 7}));

    let error = user_error(&server, response).await;

    assert!(error.is_not_found(), "{error:?}");
    assert_eq!(error.to_string(), "Resource not found");
}

#[tokio::test]
async fn known_error_codes_are_reported_with_their_message() {
    let server = MockServer::start().await;
    let response =
        ResponseTemplate::new(401).set_body_json(json!({"error": "bad_token", "error_code": 8}));

    let error = user_error(&server, response).await;

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert_eq!(error.to_string(), "Bad token");
}

#[tokio::test]
async fn unknown_error_codes_keep_the_provider_text() {
    let server = MockServer::start().await;
    let response = ResponseTemplate::new(400)
        .set_body_json(json!({"error": "something_new", "error_code": 99}));

    let error = user_error(&server, response).await;

    assert_eq!(error.to_string(), "something_new");
}

#[tokio::test]
async fn error_bodies_without_error_field_are_reported_verbatim() {
    let server = MockServer::start().await;
    let response = ResponseTemplate::new(503).set_body_string("<html>maintenance</html>");

    let error = user_error(&server, response).await;

    assert_eq!(error.to_string(), "<html>maintenance</html>");
}

#[tokio::test]
async fn empty_error_bodies_report_the_http_status() {
    let server = MockServer::start().await;

    let error = user_error(&server, ResponseTemplate::new(502)).await;

    assert!(error.to_string().contains("502"), "{error}");
}

#[tokio::test]
async fn requests_time_out_after_the_configured_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"username": "late"}))
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let client = client_with(&server, &config("secret", Duration::from_millis(100)));

    let error = client.user().await.expect_err("times out");

    assert!(matches!(error, DebridError::Transport(_)), "{error:?}");
}

async fn mock_server_time(server: &MockServer, time: &str) {
    Mock::given(method("GET"))
        .and(path("/rest/1.0/time/iso"))
        .respond_with(ResponseTemplate::new(200).set_body_string(time))
        .expect(1)
        .mount(server)
        .await;
}

fn listed(id: &str) -> serde_json::Value {
    json!({"id": id, "filename": format!("{id}.mkv"), "hash": format!("hash-{id}"),
           "bytes": 10, "original_bytes": 20, "host": "real-debrid.com", "split": 2000,
           "progress": 100, "status": "downloaded", "added": "2024-05-01T10:00:00.000Z",
           "ended": "2024-05-01T11:00:00.000Z", "links": [format!("https://real-debrid.com/d/{id}")]})
}

#[tokio::test]
async fn listing_requests_pages_until_an_empty_page() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T12:00:00+00:00").await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param_is_missing("offset"))
        .and(query_param("limit", "5000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([listed("A"), listed("B")])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param("offset", "5000"))
        .and(query_param("limit", "5000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([listed("C")])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param("offset", "10000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;

    let torrents = client(&server).list_torrents().await.expect("listing");

    let ids: Vec<&str> = torrents.iter().map(|torrent| torrent.id.as_str()).collect();
    assert_eq!(ids, ["A", "B", "C"]);
    let first = &torrents[0];
    assert_eq!(first.filename, "A.mkv");
    assert_eq!(first.hash, "hash-A");
    assert_eq!((first.bytes, first.original_bytes), (10, 20));
    assert_eq!(first.host.as_deref(), Some("real-debrid.com"));
    assert_eq!((first.split, first.progress), (2000, 100));
    assert_eq!(first.status.as_deref(), Some("downloaded"));
    assert_eq!(first.files, None);
    assert_eq!(
        first.links.as_deref(),
        Some(["https://real-debrid.com/d/A".to_owned()].as_slice())
    );
}

#[tokio::test]
async fn listing_without_content_is_empty() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T12:00:00+00:00").await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let torrents = client(&server).list_torrents().await.expect("listing");

    assert_eq!(torrents, Vec::new());
}

#[tokio::test]
async fn listing_corrects_times_with_the_server_offset_read_once() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T14:00:00+02:00").await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param_is_missing("offset"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([listed("A")])))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param("offset", "5000"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let client = client(&server);

    client.list_torrents().await.expect("first listing");
    let torrents = client.list_torrents().await.expect("second listing");

    assert_eq!(
        torrents[0].added,
        Some("2024-05-01T08:00:00Z".parse().expect("timestamp"))
    );
    assert_eq!(
        torrents[0].ended,
        Some("2024-05-01T09:00:00Z".parse().expect("timestamp"))
    );
}

#[tokio::test]
async fn unreadable_server_time_fails_the_listing() {
    let server = MockServer::start().await;
    mock_server_time(&server, "yesterday").await;

    let error = client(&server)
        .list_torrents()
        .await
        .expect_err("no offset");

    assert!(
        matches!(error, DebridError::InvalidResponse(_)),
        "{error:?}"
    );
}

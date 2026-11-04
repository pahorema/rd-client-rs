//! Premiumize client against a mock API.

mod common;

use std::time::Duration;

use jiff::Timestamp;
use rdc_debrid::{DebridError, Premiumize};
use wiremock::matchers::{body_string, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::API_KEY;

async fn client() -> (MockServer, Premiumize) {
    let server = MockServer::start().await;
    let client = Premiumize::new(&common::config(), common::http()).with_base_url(&server.uri());
    (server, client)
}

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "application/json")
}

async fn mount_account(server: &MockServer, body: &str) {
    Mock::given(method("GET"))
        .and(path("/account/info"))
        .and(query_param("apikey", API_KEY))
        .respond_with(json(body))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn user_reports_customer_id_and_premium_end() {
    let (server, client) = client().await;
    mount_account(
        &server,
        r#"{"status":"success","customer_id":1234,"premium_until":1761955200}"#,
    )
    .await;

    let user = client.user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("1234"));
    assert_eq!(
        user.expiration,
        Some(Timestamp::from_second(1_761_955_200).expect("ts"))
    );
    assert!(!user.premium);
}

#[tokio::test]
async fn user_without_premium_has_no_expiration() {
    let (server, client) = client().await;
    mount_account(
        &server,
        r#"{"status":"success","customer_id":"42","premium_until":false}"#,
    )
    .await;

    let user = client.user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("42"));
    assert_eq!(user.expiration, None);
}

#[tokio::test]
async fn requests_authenticate_with_the_api_key_query_only() {
    let (server, client) = client().await;
    mount_account(&server, r#"{"status":"success","customer_id":1}"#).await;

    client.user().await.expect("user");

    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.query(), Some("apikey=test-api-key"));
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn blank_api_key_is_not_configured() {
    let server = MockServer::start().await;
    let mut config = common::config();
    config.api_key = "  ".to_owned();
    let client = Premiumize::new(&config, common::http()).with_base_url(&server.uri());

    let error = client.user().await.expect_err("blank key");

    assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn error_status_becomes_a_provider_error_with_its_message() {
    let (server, client) = client().await;
    mount_account(&server, r#"{"status":"error","message":"Not logged in."}"#).await;

    let error = client.user().await.expect_err("error status");

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert_eq!(error.to_string(), "Not logged in.");
}

#[tokio::test]
async fn rate_limit_messages_become_rate_limited_for_two_minutes() {
    for message in [
        "rate limit exceeded",
        "You've made too many API requests too quickly.",
        "Your fair-use points, booster points, or active-job count is exhausted.",
        "This account's usage limit for this service has been reached.",
        "The target service is unreachable right now. Retry after a delay.",
    ] {
        let (server, client) = client().await;
        let body = serde_json::json!({"status": "error", "message": message}).to_string();
        mount_account(&server, &body).await;

        let error = client.user().await.expect_err("rate limited");

        assert_eq!(
            error.retry_after(),
            Some(Duration::from_secs(120)),
            "{message}"
        );
        assert_eq!(error.to_string(), message);
    }
}

#[tokio::test]
async fn rate_limit_codes_become_rate_limited() {
    for code in [
        "slow_down",
        "rate_limit_reached",
        "account_limit_reached",
        "service_limit_reached",
        "service_down",
        "semi_permanent_error",
    ] {
        let (server, client) = client().await;
        let body = serde_json::json!({"status": "error", "code": code, "message": "retry later"})
            .to_string();
        mount_account(&server, &body).await;

        let error = client.user().await.expect_err("rate limited");

        assert_eq!(
            error.retry_after(),
            Some(Duration::from_secs(120)),
            "{code}"
        );
    }
}

#[tokio::test]
async fn empty_and_unreadable_responses_are_invalid() {
    for response in [
        ResponseTemplate::new(204),
        ResponseTemplate::new(200).set_body_string("<html>maintenance</html>"),
        ResponseTemplate::new(502).set_body_string("Bad gateway"),
    ] {
        let server = MockServer::start().await;
        let client =
            Premiumize::new(&common::config(), common::http()).with_base_url(&server.uri());
        Mock::given(path("/account/info"))
            .respond_with(response)
            .mount(&server)
            .await;

        let error = client.user().await.expect_err("invalid");

        assert!(
            matches!(error, DebridError::InvalidResponse(_)),
            "{error:?}"
        );
    }
}

#[tokio::test]
async fn transfers_are_listed_without_sizes_or_files() {
    let (server, client) = client().await;
    Mock::given(method("POST"))
        .and(path("/transfer/list"))
        .and(query_param("apikey", API_KEY))
        .respond_with(json(
            r#"{"status":"success","transfers":[
                {"id":"t1","name":"Some.Release","message":"Downloading","status":"running",
                 "progress":0.257,"folder_id":"f1","file_id":null,"src":"magnet:?xt=urn:btih:abc"},
                {"id":"t2","name":"Other","status":"finished","progress":null,"folder_id":null}
            ]}"#,
        ))
        .mount(&server)
        .await;

    let torrents = client.list_torrents().await.expect("list");

    assert_eq!(torrents.len(), 2);
    let first = &torrents[0];
    assert_eq!(first.id, "t1");
    assert_eq!(first.filename, "Some.Release");
    assert_eq!(first.original_filename.as_deref(), Some("Some.Release"));
    assert_eq!(first.hash, "magnet:?xt=urn:btih:abc");
    assert_eq!(first.progress, 25);
    assert_eq!(first.status.as_deref(), Some("running"));
    assert_eq!(first.message.as_deref(), Some("Downloading"));
    assert_eq!((first.bytes, first.original_bytes), (0, 0));
    assert_eq!(first.files, Some(Vec::new()));
    assert_eq!(first.links, Some(vec!["f1".to_owned()]));
    assert_eq!((first.speed, first.seeders), (Some(0), Some(0)));
    assert_eq!(torrents[1].progress, 100, "missing progress counts as done");
    assert_eq!(torrents[1].links, Some(Vec::new()));
}

#[tokio::test]
async fn add_magnet_posts_the_source_form_and_returns_the_transfer_id() {
    let (server, client) = client().await;
    Mock::given(method("POST"))
        .and(path("/transfer/create"))
        .respond_with(json(
            r#"{"status":"success","id":"magnet-transfer-id","name":"test"}"#,
        ))
        .mount(&server)
        .await;

    let id = client
        .add_magnet("magnet:?xt=urn:btih:test")
        .await
        .expect("added");

    assert_eq!(id, "magnet-transfer-id");
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests[0].url.query(), Some("apikey=test-api-key"));
    assert!(!requests[0].headers.contains_key("authorization"));
    assert_eq!(
        String::from_utf8_lossy(&requests[0].body),
        "src=magnet%3A%3Fxt%3Durn%3Abtih%3Atest&folder_id="
    );
}

#[tokio::test]
async fn add_torrent_file_uploads_a_multipart_torrent() {
    let (server, client) = client().await;
    Mock::given(method("POST"))
        .and(path("/transfer/create"))
        .respond_with(json(
            r#"{"status":"success","id":"torrent-transfer-id","name":"test"}"#,
        ))
        .mount(&server)
        .await;

    let id = client
        .add_torrent_file(b"torrent body")
        .await
        .expect("added");

    assert_eq!(id, "torrent-transfer-id");
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests[0].url.query(), Some("apikey=test-api-key"));
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(
        body.contains(r#"name="file"; filename="1.torrent""#),
        "{body}"
    );
    assert!(body.contains("application/x-bittorrent"), "{body}");
    assert!(body.contains("torrent body"), "{body}");
    assert!(body.contains(r#"name="folder_id""#), "{body}");
}

#[tokio::test]
async fn add_without_a_transfer_id_fails() {
    let (server, client) = client().await;
    Mock::given(path("/transfer/create"))
        .respond_with(json(r#"{"status":"success","id":""}"#))
        .mount(&server)
        .await;

    let error = client
        .add_magnet("magnet:?xt=urn:btih:test")
        .await
        .expect_err("no id");

    assert_eq!(error.to_string(), "Unable to add magnet link");
}

#[tokio::test]
async fn delete_posts_the_transfer_id() {
    let (server, client) = client().await;
    Mock::given(method("POST"))
        .and(path("/transfer/delete"))
        .and(query_param("apikey", API_KEY))
        .and(body_string("id=t1"))
        .respond_with(json(r#"{"status":"success"}"#))
        .expect(1)
        .mount(&server)
        .await;

    client.delete_transfer("t1").await.expect("deleted");
}

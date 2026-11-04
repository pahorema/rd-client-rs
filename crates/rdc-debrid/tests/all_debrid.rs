//! AllDebrid client against a mocked API.

use std::sync::Arc;
use std::time::Duration;

use rdc_debrid::{
    AllDebrid, DebridError, ProviderConfig, ProviderHttp, RateLimitCoordinator, RetryPolicy,
};
use rdc_model::Torrent;
use serde_json::json;
use wiremock::matchers::{bearer_token, method, path, query_param};
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

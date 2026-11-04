//! Real-Debrid client against a mocked REST API.

use std::sync::Arc;
use std::time::Duration;

use rdc_debrid::{
    DebridError, ProviderConfig, ProviderHttp, RateLimitCoordinator, RealDebrid, RetryPolicy,
};
use serde_json::json;
use wiremock::matchers::{bearer_token, method, path};
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

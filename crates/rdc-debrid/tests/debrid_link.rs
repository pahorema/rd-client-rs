//! Debrid-Link client against a mock API.

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use rdc_debrid::{
    DebridError, DebridLink, ProviderConfig, ProviderHttp, RateLimitCoordinator, RetryPolicy,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(api_key: &str) -> ProviderConfig {
    ProviderConfig {
        api_key: api_key.to_owned(),
        api_hostname: None,
        timeout: Duration::from_secs(5),
        prefer_zipped_downloads: false,
    }
}

fn client_with_key(server: &MockServer, api_key: &str) -> DebridLink {
    let http = ProviderHttp::new(
        Duration::from_secs(5),
        Arc::new(RateLimitCoordinator::new()),
    )
    .expect("client builds")
    .with_retry_policy(RetryPolicy {
        max_retries: 0,
        base_delay: Duration::from_millis(1),
    });
    DebridLink::new(&config(api_key), http).with_base_url(&server.uri())
}

fn client(server: &MockServer) -> DebridLink {
    client_with_key(server, "test-key")
}

fn ok(value: &Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "success": true, "value": value }))
}

#[tokio::test]
async fn user_reports_name_and_expiry_from_remaining_premium_seconds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/account/infos"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ok(&json!({ "username": "alice", "premiumLeft": 86_400 })))
        .expect(1)
        .mount(&server)
        .await;

    let before = Timestamp::now();
    let user = client(&server).user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("alice"));
    assert!(!user.premium);
    let expiration = user.expiration.expect("premium account expires");
    let remaining = expiration.duration_since(before);
    assert!(
        remaining >= SignedDuration::from_hours(24)
            && remaining < SignedDuration::from_hours(24) + SignedDuration::from_secs(60),
        "{remaining:?}"
    );
}

#[tokio::test]
async fn user_without_premium_time_has_no_expiry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/account/infos"))
        .respond_with(ok(&json!({ "username": "bob", "premiumLeft": 0 })))
        .mount(&server)
        .await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("bob"));
    assert_eq!(user.expiration, None);
}

#[tokio::test]
async fn blank_api_key_is_not_configured_and_sends_nothing() {
    let server = MockServer::start().await;

    let error = client_with_key(&server, "  ")
        .user()
        .await
        .expect_err("no key");

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
async fn failed_envelope_reports_the_api_error_code() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({ "success": false, "error": "badToken" })),
        )
        .mount(&server)
        .await;

    let error = client(&server).user().await.expect_err("bad token");

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert!(error.to_string().contains("badToken"), "{error}");
}

#[tokio::test]
async fn error_status_without_envelope_reports_the_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad gateway</html>"))
        .mount(&server)
        .await;

    let error = client(&server).user().await.expect_err("bad gateway");

    assert!(error.to_string().contains("502"), "{error}");
}

#[tokio::test]
async fn unreadable_success_body_is_an_invalid_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let error = client(&server).user().await.expect_err("garbage");

    assert!(
        matches!(error, DebridError::InvalidResponse(_)),
        "{error:?}"
    );
}

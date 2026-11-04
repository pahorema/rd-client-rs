//! TorBox client against a mock API.

mod common;

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use rdc_debrid::{DebridError, ProviderHttp, RateLimitCoordinator, RetryPolicy, TorBox};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Retries transport failures twice without waiting.
const FAST_RETRY: RetryPolicy = RetryPolicy {
    max_retries: 2,
    base_delay: Duration::ZERO,
};

async fn client() -> (MockServer, TorBox) {
    let server = MockServer::start().await;
    let client = TorBox::new(&common::config(), common::http()).with_base_url(&server.uri());
    (server, client)
}

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "application/json")
}

async fn mount_user(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/user/me"))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn user_reports_email_and_plan_end() {
    let (server, client) = client().await;
    Mock::given(method("GET"))
        .and(path("/user/me"))
        .and(query_param("settings", "false"))
        .and(header("authorization", "Bearer test-api-key"))
        .respond_with(json(
            r#"{"success":true,"error":null,"detail":"ok","data":
                {"email":"me@example.com","plan":2,"premium_expires_at":"2025-11-01T00:00:00Z"}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let user = client.user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("me@example.com"));
    assert_eq!(
        user.expiration,
        Some(Timestamp::from_second(1_761_955_200).expect("ts"))
    );
}

#[tokio::test]
async fn free_plan_has_no_expiration() {
    let (server, client) = client().await;
    mount_user(
        &server,
        json(r#"{"success":true,"data":{"email":"me@example.com","plan":0,"premium_expires_at":"2025-11-01T00:00:00"}}"#),
    )
    .await;

    assert_eq!(client.user().await.expect("user").expiration, None);
}

#[tokio::test]
async fn blank_api_key_is_not_configured() {
    let server = MockServer::start().await;
    let mut config = common::config();
    config.api_key = String::new();
    let client = TorBox::new(&config, common::http()).with_base_url(&server.uri());

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
async fn error_envelopes_report_code_and_detail() {
    for (body, message) in [
        (
            r#"{"success":false,"error":"BAD_TOKEN","detail":"Invalid token."}"#,
            "BAD_TOKEN: Invalid token.",
        ),
        (
            r#"{"success":false,"error":"BAD_TOKEN","detail":"BAD_TOKEN: Invalid."}"#,
            "BAD_TOKEN: Invalid.",
        ),
        (
            r#"{"success":false,"error":"BAD_TOKEN","detail":null}"#,
            "BAD_TOKEN",
        ),
    ] {
        let (server, client) = client().await;
        mount_user(&server, ResponseTemplate::new(401).set_body_string(body)).await;

        let error = client.user().await.expect_err("error envelope");

        assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
        assert_eq!(error.to_string(), message);
    }
}

#[tokio::test]
async fn missing_items_are_not_found() {
    let (server, client) = client().await;
    mount_user(
        &server,
        json(r#"{"success":false,"error":"ITEM_NOT_FOUND","detail":"Gone."}"#),
    )
    .await;

    let error = client.user().await.expect_err("missing");

    assert!(error.is_not_found(), "{error:?}");
}

#[tokio::test]
async fn limit_errors_are_rate_limits_that_block_the_host() {
    for body in [
        r#"{"success":false,"error":"ACTIVE_LIMIT","detail":"Active limit reached"}"#,
        r#"{"success":false,"error":"rate_limit","detail":"Too fast"}"#,
        r#"{"success":true,"detail":"You hit the rate limit of 60 per minute"}"#,
        r#"{"success":false,"error":"PLAN","detail":"Only 10 per 1 hour"}"#,
    ] {
        let (server, client) = client().await;
        mount_user(&server, json(body)).await;

        let error = client.user().await.expect_err("rate limited");

        assert_eq!(
            error.retry_after(),
            Some(Duration::from_secs(120)),
            "{body}"
        );
        let second = client.user().await.expect_err("cooling down");
        assert!(second.to_string().contains("cooldown"), "{second}");
        assert_eq!(server.received_requests().await.expect("recorded").len(), 1);
    }
}

#[tokio::test]
async fn slow_down_messages_are_rate_limits() {
    let coordinator = Arc::new(RateLimitCoordinator::new());
    let server = MockServer::start().await;
    let http = ProviderHttp::new(Duration::from_secs(5), Arc::clone(&coordinator))
        .expect("client builds")
        .with_retry_policy(FAST_RETRY);
    let client = TorBox::new(&common::config(), http).with_base_url(&server.uri());
    mount_user(
        &server,
        ResponseTemplate::new(503).set_body_string("slow_down"),
    )
    .await;

    let error = client.user().await.expect_err("rate limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(120)));
    let host = server.address().ip().to_string();
    assert!(coordinator.remaining_cooldown(&host) > Duration::from_secs(110));
}

#[tokio::test]
async fn too_many_requests_is_a_rate_limit() {
    let (server, client) = client().await;
    mount_user(
        &server,
        ResponseTemplate::new(429)
            .insert_header("Retry-After", "30")
            .set_body_string(r#"{"success":false,"error":"RATE_LIMIT"}"#),
    )
    .await;

    let error = client.user().await.expect_err("rate limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(30)));
}

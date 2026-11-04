//! TorBox client against a mock API.

mod common;

use jiff::Timestamp;
use rdc_debrid::{DebridError, TorBox};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

use std::time::Duration;

use rdc_aria2::{Client, Error};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn rpc_url(server: &MockServer) -> String {
    format!("{}/jsonrpc", server.uri())
}

fn client(server: &MockServer, secret: &str) -> Client {
    Client::builder(rpc_url(server))
        .secret(secret)
        .retry_delay(Duration::from_millis(1))
        .build()
        .expect("valid client configuration")
}

fn request(rpc_method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": "rd-client", "method": rpc_method, "params": params})
}

fn result(value: &Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(json!({"jsonrpc": "2.0", "id": "rd-client", "result": value}))
}

async fn expect_call(
    server: &MockServer,
    rpc_method: &str,
    params: Value,
    response: ResponseTemplate,
) {
    Mock::given(method("POST"))
        .and(path("/jsonrpc"))
        .and(body_json(request(rpc_method, &params)))
        .respond_with(response)
        .expect(1)
        .mount(server)
        .await;
}

#[test]
fn builder_rejects_url_without_jsonrpc_suffix() {
    let error = Client::builder("http://127.0.0.1:6800/rpc")
        .build()
        .unwrap_err();

    assert!(matches!(error, Error::InvalidUrl));
    assert_eq!(error.to_string(), "The URL must end with /jsonrpc");
}

#[tokio::test]
async fn version_sends_token_and_parses_result() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.getVersion",
        json!(["token:secret"]),
        result(&json!({"version": "1.37.0", "enabledFeatures": ["Async DNS", "HTTPS"]})),
    )
    .await;

    let version = client(&server, "secret").version().await.unwrap();

    assert_eq!(version.version, "1.37.0");
    assert_eq!(version.enabled_features, ["Async DNS", "HTTPS"]);
}

#[tokio::test]
async fn blank_secret_sends_no_token_parameter() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.getVersion",
        json!([]),
        result(&json!({"version": "1.37.0"})),
    )
    .await;

    let version = client(&server, "   ").version().await.unwrap();

    assert_eq!(version.version, "1.37.0");
    assert_eq!(version.enabled_features.len(), 0);
}

#[tokio::test]
async fn missing_secret_sends_no_token_parameter() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.getVersion",
        json!([]),
        result(&json!({"version": "1.37.0"})),
    )
    .await;

    let client = Client::builder(rpc_url(&server)).build().unwrap();

    client.version().await.unwrap();
}

#[tokio::test]
async fn requests_carry_the_rd_client_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header(
            "user-agent",
            concat!("rd-client/", env!("CARGO_PKG_VERSION")),
        ))
        .respond_with(result(&json!({"version": "1.37.0"})))
        .expect(1)
        .mount(&server)
        .await;

    client(&server, "secret").version().await.unwrap();
}

#[tokio::test]
async fn rpc_error_with_error_status_is_typed_and_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "jsonrpc": "2.0", "id": "rd-client", "error": {"code": 1, "message": "Unauthorized"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder(rpc_url(&server))
        .secret("wrong")
        .retry_count(3)
        .retry_delay(Duration::from_millis(1))
        .build()
        .unwrap();

    let error = client.version().await.unwrap_err();

    assert!(
        matches!(&error, Error::Rpc { code: 1, message } if message == "Unauthorized"),
        "{error:?}"
    );
    assert_eq!(error.rpc_code(), Some(1));
    assert_eq!(error.to_string(), "aria2 error 1: Unauthorized");
}

#[tokio::test]
async fn rpc_error_with_success_status_is_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": "rd-client", "error": {"code": 1, "message": "GID 2089b05ecca3d829 is not found"}
        })))
        .mount(&server)
        .await;

    let error = client(&server, "secret").version().await.unwrap_err();

    assert!(matches!(error, Error::Rpc { code: 1, .. }), "{error:?}");
}

#[tokio::test]
async fn error_status_without_rpc_error_reports_status_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(502).set_body_string("bad gateway"))
        .expect(1)
        .mount(&server)
        .await;

    let error = client(&server, "secret").version().await.unwrap_err();

    assert!(
        matches!(&error, Error::Status { status: 502, body } if body == "bad gateway"),
        "{error:?}"
    );
}

#[tokio::test]
async fn empty_body_is_reported_as_empty_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let error = client(&server, "secret").version().await.unwrap_err();

    assert!(matches!(error, Error::EmptyResponse), "{error:?}");
}

#[tokio::test]
async fn response_without_result_is_reported_as_empty_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc": "2.0", "id": "rd-client"})),
        )
        .mount(&server)
        .await;

    let error = client(&server, "secret").version().await.unwrap_err();

    assert!(matches!(error, Error::EmptyResponse), "{error:?}");
}

#[tokio::test]
async fn malformed_body_is_reported_with_the_raw_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;

    let error = client(&server, "secret").version().await.unwrap_err();

    assert!(
        matches!(&error, Error::Decode { body, .. } if body == "<html>"),
        "{error:?}"
    );
}

#[tokio::test]
async fn transport_failure_is_retried_until_it_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(result(&json!({"version": "slow"})).set_delay(Duration::from_secs(5)))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(result(&json!({"version": "1.37.0"})))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder(rpc_url(&server))
        .timeout(Duration::from_millis(200))
        .retry_count(2)
        .retry_delay(Duration::from_millis(1))
        .build()
        .unwrap();

    let version = client.version().await.unwrap();

    assert_eq!(version.version, "1.37.0");
}

#[tokio::test]
async fn transport_failure_after_all_retries_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(result(&json!({"version": "slow"})).set_delay(Duration::from_secs(5)))
        .expect(2)
        .mount(&server)
        .await;
    let client = Client::builder(rpc_url(&server))
        .timeout(Duration::from_millis(200))
        .retry_count(1)
        .retry_delay(Duration::from_millis(1))
        .build()
        .unwrap();

    let error = client.version().await.unwrap_err();

    assert!(matches!(error, Error::Transport(_)), "{error:?}");
}

#[test]
fn debug_output_redacts_the_secret() {
    let client = Client::builder("http://127.0.0.1:6800/jsonrpc")
        .secret("hunter2")
        .build()
        .unwrap();

    let debug = format!("{client:?}");

    assert!(!debug.contains("hunter2"), "{debug}");
}

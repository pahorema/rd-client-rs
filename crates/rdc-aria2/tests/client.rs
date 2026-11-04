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

#[test]
fn debug_output_redacts_the_secret() {
    let client = Client::builder("http://127.0.0.1:6800/jsonrpc")
        .secret("hunter2")
        .build()
        .unwrap();

    let debug = format!("{client:?}");

    assert!(!debug.contains("hunter2"), "{debug}");
}

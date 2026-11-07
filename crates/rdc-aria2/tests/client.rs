use std::time::Duration;

use rdc_aria2::{AddUriOptions, Client, DownloadState, Error};
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

fn status_json(gid: &str, status: &str, uri: &str) -> Value {
    json!({
        "gid": gid,
        "status": status,
        "totalLength": "1048576",
        "completedLength": "524288",
        "downloadSpeed": "2048",
        "uploadSpeed": "0",
        "connections": "1",
        "dir": "/downloads/Some.Release",
        "errorCode": "0",
        "files": [{
            "index": "1",
            "path": "/downloads/Some.Release/file.mkv",
            "length": "1048576",
            "completedLength": "524288",
            "selected": "true",
            "uris": [{"status": "used", "uri": uri}]
        }]
    })
}

#[tokio::test]
async fn tell_status_parses_the_download() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.tellStatus",
        json!(["token:secret", "2089b05ecca3d829"]),
        result(&status_json(
            "2089b05ecca3d829",
            "active",
            "https://host/file.mkv",
        )),
    )
    .await;

    let status = client(&server, "secret")
        .tell_status("2089b05ecca3d829")
        .await
        .unwrap();

    assert_eq!(status.gid, "2089b05ecca3d829");
    assert_eq!(status.status, DownloadState::Active);
    assert_eq!(status.total_length, 1_048_576);
    assert_eq!(status.completed_length, 524_288);
    assert_eq!(status.download_speed, 2048);
    assert_eq!(status.error_code.as_deref(), Some("0"));
    assert_eq!(status.error_message, None);
    assert_eq!(status.dir, "/downloads/Some.Release");
    assert_eq!(status.files.len(), 1);
    assert_eq!(status.files[0].path, "/downloads/Some.Release/file.mkv");
    assert_eq!(status.files[0].uris[0].uri, "https://host/file.mkv");
    assert_eq!(status.files[0].uris[0].status, "used");
    assert!(status.has_uri("https://host/file.mkv"));
    assert!(!status.has_uri("https://host/other.mkv"));
}

#[tokio::test]
async fn tell_status_reports_error_details() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.tellStatus",
        json!(["token:secret", "abc"]),
        result(&json!({
            "gid": "abc", "status": "error", "totalLength": "0", "completedLength": "0",
            "downloadSpeed": "0", "errorCode": "3", "errorMessage": "Resource not found", "dir": "/d", "files": []
        })),
    )
    .await;

    let status = client(&server, "secret").tell_status("abc").await.unwrap();

    assert_eq!(status.status, DownloadState::Error);
    assert_eq!(status.error_code.as_deref(), Some("3"));
    assert_eq!(status.error_message.as_deref(), Some("Resource not found"));
}

#[tokio::test]
async fn tell_active_sends_only_the_token() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.tellActive",
        json!(["token:secret"]),
        result(&json!([status_json("a", "active", "https://host/a")])),
    )
    .await;

    let active = client(&server, "secret").tell_active().await.unwrap();

    assert_eq!(active.len(), 1);
    assert_eq!(active[0].gid, "a");
}

#[tokio::test]
async fn tell_waiting_sends_offset_and_count() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.tellWaiting",
        json!(["token:secret", 0, 1000]),
        result(&json!([status_json("w", "paused", "https://host/w")])),
    )
    .await;

    let waiting = client(&server, "secret")
        .tell_waiting(0, 1000)
        .await
        .unwrap();

    assert_eq!(waiting[0].status, DownloadState::Paused);
}

#[tokio::test]
async fn tell_stopped_sends_offset_and_count() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.tellStopped",
        json!(["token:secret", -1, 10]),
        result(&json!([status_json("s", "complete", "https://host/s")])),
    )
    .await;

    let stopped = client(&server, "secret")
        .tell_stopped(-1, 10)
        .await
        .unwrap();

    assert_eq!(stopped[0].status, DownloadState::Complete);
}

fn multicall_request(token: Option<&str>) -> Value {
    let params = |args: Vec<Value>| {
        let mut params: Vec<Value> = token.map(|token| json!(token)).into_iter().collect();
        params.extend(args);
        params
    };
    json!([[
        {"methodName": "aria2.tellStopped", "params": params(vec![json!(0), json!(1000)])},
        {"methodName": "aria2.tellWaiting", "params": params(vec![json!(0), json!(1000)])},
        {"methodName": "aria2.tellActive", "params": params(Vec::new())},
    ]])
}

#[tokio::test]
async fn tell_all_batches_stopped_waiting_and_active_in_one_multicall() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "system.multicall",
        multicall_request(Some("token:secret")),
        result(&json!([
            [[
                status_json("s1", "complete", "https://host/s1"),
                status_json("s2", "removed", "https://host/s2")
            ]],
            [[status_json("w1", "waiting", "https://host/w1")]],
            [[status_json("a1", "active", "https://host/a1")]],
        ])),
    )
    .await;

    let all = client(&server, "secret").tell_all().await.unwrap();

    let gids: Vec<&str> = all.iter().map(|status| status.gid.as_str()).collect();
    assert_eq!(gids, ["s1", "s2", "w1", "a1"]);
}

#[tokio::test]
async fn tell_all_without_secret_sends_no_token_in_any_call() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "system.multicall",
        multicall_request(None),
        result(&json!([[[]], [[]], [[]]])),
    )
    .await;

    let all = client(&server, "").tell_all().await.unwrap();

    assert_eq!(all.len(), 0);
}

#[tokio::test]
async fn tell_all_reports_a_failed_call_in_the_batch() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(result(&json!([
            [[]],
            {"code": 1, "message": "Unauthorized"},
            [[]],
        ])))
        .mount(&server)
        .await;

    let error = client(&server, "secret").tell_all().await.unwrap_err();

    assert!(
        matches!(&error, Error::Rpc { code: 1, message } if message == "Unauthorized"),
        "{error:?}"
    );
}

#[tokio::test]
async fn add_uri_sends_uris_and_options_and_returns_the_gid() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.addUri",
        json!(["token:secret", ["https://host/file.mkv"], {"dir": "/downloads/Some.Release", "out": "file.mkv"}]),
        result(&json!("2089b05ecca3d829")),
    )
    .await;
    let options = AddUriOptions {
        dir: Some("/downloads/Some.Release".to_owned()),
        out: Some("file.mkv".to_owned()),
    };

    let gid = client(&server, "secret")
        .add_uri(&["https://host/file.mkv"], &options)
        .await
        .unwrap();

    assert_eq!(gid, "2089b05ecca3d829");
}

#[tokio::test]
async fn add_uri_without_options_sends_an_empty_object() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.addUri",
        json!(["token:secret", ["https://host/file.mkv"], {}]),
        result(&json!("gid")),
    )
    .await;

    client(&server, "secret")
        .add_uri(&["https://host/file.mkv"], &AddUriOptions::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn add_uri_rejects_an_empty_uri_list_without_calling_aria2() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(result(&json!("gid")))
        .expect(0)
        .mount(&server)
        .await;

    let error = client(&server, "secret")
        .add_uri(&[], &AddUriOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(error, Error::EmptyUriList), "{error:?}");
}

#[tokio::test]
async fn add_uri_that_may_have_reached_aria2_is_not_sent_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(result(&json!("gid")).set_delay(Duration::from_secs(5)))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder(rpc_url(&server))
        .timeout(Duration::from_millis(200))
        .retry_count(3)
        .retry_delay(Duration::from_millis(1))
        .build()
        .unwrap();

    let error = client
        .add_uri(&["https://host/file.mkv"], &AddUriOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Transport(_)), "{error:?}");
}

#[tokio::test]
async fn force_remove_sends_the_gid() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.forceRemove",
        json!(["token:secret", "abc"]),
        result(&json!("abc")),
    )
    .await;

    let gid = client(&server, "secret").force_remove("abc").await.unwrap();

    assert_eq!(gid, "abc");
}

#[tokio::test]
async fn remove_download_result_sends_the_gid() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.removeDownloadResult",
        json!(["token:secret", "abc"]),
        result(&json!("OK")),
    )
    .await;

    client(&server, "secret")
        .remove_download_result("abc")
        .await
        .unwrap();
}

#[tokio::test]
async fn pause_sends_the_gid() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.pause",
        json!(["token:secret", "abc"]),
        result(&json!("abc")),
    )
    .await;

    let gid = client(&server, "secret").pause("abc").await.unwrap();

    assert_eq!(gid, "abc");
}

#[tokio::test]
async fn unpause_sends_the_gid() {
    let server = MockServer::start().await;
    expect_call(
        &server,
        "aria2.unpause",
        json!(["token:secret", "abc"]),
        result(&json!("abc")),
    )
    .await;

    let gid = client(&server, "secret").unpause("abc").await.unwrap();

    assert_eq!(gid, "abc");
}

#[tokio::test]
async fn unknown_gid_is_reported_as_rpc_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "jsonrpc": "2.0", "id": "rd-client", "error": {"code": 1, "message": "Active Download not found for GID#abc"}
        })))
        .mount(&server)
        .await;

    let error = client(&server, "secret")
        .force_remove("abc")
        .await
        .unwrap_err();

    assert_eq!(error.rpc_code(), Some(1));
}

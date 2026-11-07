use std::time::Duration;

use rdc_synology::{Client, Error};
use serde_json::json;
use wiremock::matchers::{
    body_string_contains, header, method, path, query_param, query_param_is_missing,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{SID, failure, logged_in, mount_info, mount_login, ok, ok_without_data};

#[tokio::test]
async fn connect_rejects_an_invalid_url_without_a_request() {
    let error = Client::connect("  ").await.unwrap_err();

    assert!(matches!(error, Error::InvalidUrl(_)), "{error:?}");
}

#[tokio::test]
async fn connect_queries_every_api_without_a_session() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/webapi/query.cgi"))
        .and(query_param("api", "SYNO.API.Info"))
        .and(query_param("version", "1"))
        .and(query_param("method", "query"))
        .and(query_param("query", "all"))
        .and(query_param_is_missing("_sid"))
        .and(query_param_is_missing("session"))
        .and(header(
            "user-agent",
            concat!("rd-client/", env!("CARGO_PKG_VERSION")),
        ))
        .respond_with(ok(&json!({})))
        .expect(1)
        .mount(&server)
        .await;

    Client::connect(&format!("{}/", server.uri()))
        .await
        .unwrap();
}

#[tokio::test]
async fn connect_uses_the_discovered_api_paths() {
    let server = MockServer::start().await;
    Mock::given(path("/webapi/query.cgi"))
        .respond_with(ok(
            &json!({"SYNO.API.Auth": {"path": "auth.cgi", "minVersion": 1, "maxVersion": 7}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/webapi/auth.cgi"))
        .and(body_string_contains("version=6"))
        .respond_with(ok(&json!({"sid": "abc"})))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::connect(&server.uri()).await.unwrap();

    client.login("user", "pass").await.unwrap();
}

#[tokio::test]
async fn connect_reports_a_failed_discovery() {
    let server = MockServer::start().await;
    Mock::given(path("/webapi/query.cgi"))
        .respond_with(failure(102))
        .mount(&server)
        .await;

    let error = Client::connect(&server.uri()).await.unwrap_err();

    let Error::Api(api) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(api.api, "SYNO.API.Info");
    assert_eq!(api.method, "query");
    assert_eq!(api.code, 102);
    assert_eq!(api.description, "The requested API does not exist");
}

#[tokio::test]
async fn login_posts_the_credentials_in_the_body() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    Mock::given(method("POST"))
        .and(path("/webapi/entry.cgi"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string_contains("api=SYNO.API.Auth"))
        .and(body_string_contains("version=6"))
        .and(body_string_contains("method=login"))
        .and(body_string_contains("account=user"))
        .and(body_string_contains("passwd=p%40ss+word%26"))
        .and(body_string_contains("format=sid"))
        .and(query_param_is_missing("passwd"))
        .and(query_param_is_missing("account"))
        .respond_with(ok(&json!({"sid": SID, "is_portal_port": false})))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::connect(&server.uri()).await.unwrap();

    client.login("user", "p@ss word&").await.unwrap();
}

#[tokio::test]
async fn login_rejected_by_dsm_reports_code_and_description() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    Mock::given(method("POST"))
        .respond_with(failure(400))
        .mount(&server)
        .await;
    let client = Client::connect(&server.uri()).await.unwrap();

    let error = client.login("user", "wrong").await.unwrap_err();

    assert_eq!(error.api_code(), Some(400));
    assert_eq!(
        error.to_string(),
        "SYNO.API.Auth.login (version 6) failed with error 400: No such account or incorrect password"
    );
}

#[tokio::test]
async fn login_requiring_two_step_verification_is_reported() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    Mock::given(method("POST"))
        .respond_with(failure(403))
        .mount(&server)
        .await;
    let client = Client::connect(&server.uri()).await.unwrap();

    let error = client.login("user", "pass").await.unwrap_err();

    let Error::Api(api) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(api.description, "2-step verification code required");
}

#[tokio::test]
async fn login_rejects_blank_credentials_without_a_request() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    Mock::given(method("POST"))
        .respond_with(ok(&json!({"sid": SID})))
        .expect(0)
        .mount(&server)
        .await;
    let client = Client::connect(&server.uri()).await.unwrap();

    for (account, password) in [("", "pass"), ("user", " ")] {
        let error = client.login(account, password).await.unwrap_err();

        assert!(matches!(error, Error::InvalidArgument(_)), "{error:?}");
    }
}

#[tokio::test]
async fn logout_sends_the_session_id() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    Mock::given(method("GET"))
        .and(path("/webapi/entry.cgi"))
        .and(query_param("api", "SYNO.API.Auth"))
        .and(query_param("version", "6"))
        .and(query_param("method", "logout"))
        .and(query_param("_sid", SID))
        .respond_with(ok_without_data())
        .expect(1)
        .mount(&server)
        .await;

    session.logout().await.unwrap();
}

#[tokio::test]
async fn non_ok_status_is_reported() {
    let server = MockServer::start().await;
    Mock::given(path("/webapi/query.cgi"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;

    let error = Client::connect(&server.uri()).await.unwrap_err();

    assert!(matches!(error, Error::UnexpectedStatus(502)), "{error:?}");
}

#[tokio::test]
async fn malformed_body_is_reported_with_the_raw_text() {
    let server = MockServer::start().await;
    Mock::given(path("/webapi/query.cgi"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;

    let error = Client::connect(&server.uri()).await.unwrap_err();

    assert!(
        matches!(&error, Error::Decode { body, .. } if body == "<html>"),
        "{error:?}"
    );
}

#[tokio::test]
async fn error_details_are_kept() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": false,
            "error": {"code": 400, "errors": [{"code": 401, "path": "/x"}]}
        })))
        .mount(&server)
        .await;

    let error = Client::connect(&server.uri())
        .await
        .unwrap()
        .login("u", "p")
        .await
        .unwrap_err();

    let Error::Api(api) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(api.errors.len(), 1);
    assert_eq!(api.errors[0].code, 401);
    assert_eq!(api.errors[0].path.as_deref(), Some("/x"));
}

#[tokio::test]
async fn transport_errors_hide_the_session_id() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    mount_login(&server, SID).await;
    Mock::given(method("GET"))
        .and(query_param("method", "logout"))
        .respond_with(ok_without_data().set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let session = Client::connect_with_timeout(&server.uri(), Duration::from_millis(200))
        .await
        .unwrap()
        .login("user", "pass")
        .await
        .unwrap();

    let error = session.logout().await.unwrap_err();

    let message = error.to_string();
    assert!(matches!(error, Error::Transport(_)), "{error:?}");
    assert!(!message.contains(SID), "{message}");
    assert!(!format!("{error:?}").contains(SID), "{error:?}");
    assert!(message.contains("timed out"), "{message}");
}

#[test]
fn debug_output_redacts_the_session_id() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let debug = runtime.block_on(async {
        let server = MockServer::start().await;
        format!("{:?}", logged_in(&server).await)
    });

    assert!(!debug.contains(SID), "{debug}");
}

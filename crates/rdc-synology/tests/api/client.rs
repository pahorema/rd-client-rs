use rdc_synology::{Client, Error};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{failure, ok};

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

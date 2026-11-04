use rdc_synology::{Client, Session};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Session id handed out by [`mount_login`].
pub(crate) const SID: &str = "sid-1";

pub(crate) fn ok(data: &Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": data}))
}

pub(crate) fn ok_without_data() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"success": true}))
}

pub(crate) fn failure(code: i64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"success": false, "error": {"code": code}}))
}

/// Answers the API discovery query with the default locations.
pub(crate) async fn mount_info(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/webapi/query.cgi"))
        .and(query_param("api", "SYNO.API.Info"))
        .and(query_param("version", "1"))
        .and(query_param("method", "query"))
        .and(query_param("query", "all"))
        .respond_with(ok(&json!({
            "SYNO.API.Info": {"path": "query.cgi", "minVersion": 1, "maxVersion": 1},
            "SYNO.API.Auth": {"path": "entry.cgi", "minVersion": 1, "maxVersion": 7},
            "SYNO.DownloadStation2.Task": {"path": "DownloadStation/task.cgi", "minVersion": 1, "maxVersion": 2},
            "SYNO.DownloadStation.Info": {"path": "DownloadStation/info.cgi", "minVersion": 1, "maxVersion": 2},
            "SYNO.FileStation.CreateFolder": {"path": "entry.cgi", "minVersion": 1, "maxVersion": 2}
        })))
        .mount(server)
        .await;
}

/// Accepts any login and hands out `sid`.
pub(crate) async fn mount_login(server: &MockServer, sid: &str) {
    Mock::given(method("POST"))
        .and(path("/webapi/entry.cgi"))
        .and(body_string_contains("method=login"))
        .respond_with(ok(&json!({"sid": sid, "is_portal_port": false})))
        .mount(server)
        .await;
}

/// A session logged in to `server` with [`SID`].
pub(crate) async fn logged_in(server: &MockServer) -> Session {
    mount_info(server).await;
    mount_login(server, SID).await;
    let client = Client::connect(&server.uri())
        .await
        .expect("discovery succeeds");
    client.login("user", "pass").await.expect("login succeeds")
}

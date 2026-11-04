use rdc_synology::{Error, Folder};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{SID, logged_in, ok};

fn create_folder_call() -> wiremock::MockBuilder {
    Mock::given(method("GET"))
        .and(path("/webapi/entry.cgi"))
        .and(query_param("api", "SYNO.FileStation.CreateFolder"))
        .and(query_param("version", "2"))
        .and(query_param("method", "create"))
        .and(query_param("session", "FileStation"))
        .and(query_param("_sid", SID))
}

#[tokio::test]
async fn create_folder_splits_the_path_and_creates_parents() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    create_folder_call()
        .and(query_param(
            "folder_path",
            r#"["/Media/Downloads/Torrents"]"#,
        ))
        .and(query_param("name", r#"["MyTorrent"]"#))
        .and(query_param("force_parent", "true"))
        .and(query_param("additional", r#"["real_path","owner","time"]"#))
        .respond_with(ok(&json!({"folders": [
            {"isdir": true, "name": "MyTorrent", "path": "/Media/Downloads/Torrents/MyTorrent"}
        ]})))
        .expect(1)
        .mount(&server)
        .await;

    let folders = session
        .create_folder("/Media/Downloads/Torrents/MyTorrent")
        .await
        .unwrap();

    assert_eq!(
        folders,
        [Folder {
            isdir: true,
            name: Some("MyTorrent".to_owned()),
            path: Some("/Media/Downloads/Torrents/MyTorrent".to_owned()),
        }]
    );
}

#[tokio::test]
async fn create_folder_rejects_a_path_without_separator() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    create_folder_call()
        .respond_with(ok(&json!({})))
        .expect(0)
        .mount(&server)
        .await;

    let error = session.create_folder("Media").await.unwrap_err();

    assert!(matches!(error, Error::InvalidArgument(_)), "{error:?}");
}

#[tokio::test]
async fn create_folder_failure_reports_file_station_details() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    create_folder_call()
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": false,
            "error": {"code": 1100, "errors": [{"code": 407, "path": "/Media/Downloads"}]}
        })))
        .mount(&server)
        .await;

    let error = session
        .create_folder("/Media/Downloads/X")
        .await
        .unwrap_err();

    let Error::Api(api) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(api.code, 1100);
    assert_eq!(api.errors[0].description, "Operation not permitted");
    assert_eq!(api.errors[0].path.as_deref(), Some("/Media/Downloads"));
}

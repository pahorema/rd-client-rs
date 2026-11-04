use rdc_synology::{Error, TaskStatus};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer};

use crate::common::{SID, failure, logged_in, ok};

fn task_json(id: &str, uri: &str) -> Value {
    json!({
        "id": id,
        "type": "https",
        "username": "user",
        "title": "file.mkv",
        "size": 1000,
        "status": 2,
        "status_extra": {"error_detail": ""},
        "additional": {
            "detail": {
                "uri": uri,
                "destination": "Media/Downloads/Some.Release",
                "create_time": 1_700_000_000,
                "started_time": 1_700_000_005,
                "completed_time": 0
            },
            "file": [{"filename": "file.mkv", "index": 0, "priority": "normal", "size": 1000, "size_downloaded": 250, "wanted": true}],
            "transfer": {"downloaded_pieces": 0, "size_downloaded": 250, "size_uploaded": 0, "speed_download": 50, "speed_upload": 0}
        }
    })
}

fn task_call(task_method: &str) -> wiremock::MockBuilder {
    Mock::given(method("GET"))
        .and(path("/webapi/DownloadStation/task.cgi"))
        .and(query_param("api", "SYNO.DownloadStation2.Task"))
        .and(query_param("version", "1"))
        .and(query_param("method", task_method))
        .and(query_param("session", "DownloadStation"))
        .and(query_param("_sid", SID))
}

#[tokio::test]
async fn list_tasks_requests_details_and_parses_every_task() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    task_call("list")
        .and(query_param("additional", r#"["detail","file","transfer"]"#))
        .respond_with(ok(&json!({
            "offset": 0,
            "total": 2,
            "task": [task_json("dbid_1", "https://host/a.mkv"), task_json("dbid_2", "https://host/b.mkv")]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tasks = session.list_tasks().await.unwrap();

    assert_eq!(tasks.len(), 2);
    let task = &tasks[0];
    assert_eq!(task.id, "dbid_1");
    assert_eq!(task.kind.as_deref(), Some("https"));
    assert_eq!(task.title.as_deref(), Some("file.mkv"));
    assert_eq!(task.size, 1000);
    assert_eq!(task.status, TaskStatus::Downloading);
    assert_eq!(task.error_detail(), None);
    assert_eq!(task.uri(), Some("https://host/a.mkv"));
    assert_eq!(task.size_downloaded(), 250);
    assert_eq!(task.speed_download(), 50);
    let additional = task.additional.as_ref().unwrap();
    assert_eq!(
        additional.detail.as_ref().unwrap().destination.as_deref(),
        Some("Media/Downloads/Some.Release")
    );
    assert_eq!(additional.file[0].filename.as_deref(), Some("file.mkv"));
    assert!(additional.file[0].wanted);
    assert_eq!(tasks[1].uri(), Some("https://host/b.mkv"));
}

#[tokio::test]
async fn list_tasks_tolerates_a_non_string_error_detail() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    task_call("list")
        .respond_with(ok(
            &json!({"task": [{"id": "dbid_1", "status": 5, "status_extra": {"error_detail": 3}}]}),
        ))
        .mount(&server)
        .await;

    let tasks = session.list_tasks().await.unwrap();

    assert_eq!(tasks[0].error_detail(), Some("3"));
    assert_eq!(tasks[0].status, TaskStatus::Finished);
}

#[tokio::test]
async fn list_tasks_without_tasks_is_empty() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    task_call("list")
        .respond_with(ok(&json!({"offset": 0, "total": 0})))
        .mount(&server)
        .await;

    assert_eq!(session.list_tasks().await.unwrap().len(), 0);
}

#[tokio::test]
async fn task_requests_one_id_and_returns_the_first_task() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    task_call("get")
        .and(query_param("id", r#"["dbid_1"]"#))
        .and(query_param("additional", r#"["detail","file","transfer"]"#))
        .respond_with(ok(
            &json!({"tasks": [task_json("dbid_1", "https://host/a.mkv")]}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let task = session.task("dbid_1").await.unwrap();

    assert_eq!(task.id, "dbid_1");
    assert_eq!(task.uri(), Some("https://host/a.mkv"));
}

#[tokio::test]
async fn task_without_a_result_is_not_found() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    task_call("get")
        .respond_with(ok(&json!({"tasks": []})))
        .mount(&server)
        .await;

    let error = session.task("dbid_9").await.unwrap_err();

    assert!(
        matches!(&error, Error::TaskNotFound(id) if id == "dbid_9"),
        "{error:?}"
    );
}

#[tokio::test]
async fn task_errors_use_the_task_descriptions() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    task_call("get")
        .respond_with(failure(404))
        .mount(&server)
        .await;

    let error = session.task("dbid_9").await.unwrap_err();

    let Error::Api(api) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(api.api, "SYNO.DownloadStation2.Task");
    assert_eq!(api.method, "get");
    assert_eq!(api.description, "Invalid task id");
}

#[tokio::test]
async fn expired_session_is_reported_as_session_error() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    task_call("list")
        .respond_with(failure(119))
        .mount(&server)
        .await;

    let error = session.list_tasks().await.unwrap_err();

    assert!(error.is_session_error(), "{error:?}");
    assert!(error.to_string().contains("SID not found"), "{error}");
}

//! Behaviour of the Download Station engine against a scripted DSM.

pub mod dsm;

use std::time::Duration;

use dsm::{FOLDER_CREATE, FakeDsm, GID, Reply, TASK_CREATE, TASK_GET, TASK_LIST, URL, task, tasks};
use rdc_download::{
    CANCELLED_MESSAGE, DownloadEngine, DownloadStationConfig, DownloadStationDownload,
    DownloadStationRequest,
};
use rdc_synology::SessionProvider;
use serde_json::json;

fn config(fake: &FakeDsm) -> DownloadStationConfig {
    DownloadStationConfig {
        url: Some(fake.server.uri()),
        username: Some("user".to_owned()),
        password: Some("pass".to_owned()),
        download_path: Some("/Media/Downloads/Torrents".to_owned()),
    }
}

fn request(fake: &FakeDsm, gid: Option<&str>) -> DownloadStationRequest {
    DownloadStationRequest {
        url: URL.to_owned(),
        file_path: fake.local.clone(),
        relative_path: "MyTorrent/file.mkv".to_owned(),
        category: None,
        gid: gid.map(str::to_owned),
    }
}

fn engine(fake: &FakeDsm, request: DownloadStationRequest) -> DownloadStationDownload {
    engine_with(request, config(fake))
}

fn engine_with(
    request: DownloadStationRequest,
    config: DownloadStationConfig,
) -> DownloadStationDownload {
    DownloadStationDownload::new(request, config, SessionProvider::new())
        .with_retry_step(Duration::from_millis(1))
}

#[tokio::test]
async fn a_missing_remote_path_fails_without_task_calls() {
    let fake = FakeDsm::start().await;
    fake.config.always(Reply::Data(json!({})));
    let config = DownloadStationConfig {
        download_path: None,
        ..config(&fake)
    };
    let engine = engine_with(request(&fake, Some(GID)), config);
    let error = engine.start().await.expect_err("no remote path");
    let message = format!("Invalid file path {}", fake.local.display());
    assert_eq!(error.message(), message);
    assert_eq!(engine.completion().outcome(), Some(Err(message)));
    assert_eq!(fake.task_calls().await, Vec::<String>::new());
}

#[tokio::test]
async fn an_existing_task_is_adopted() {
    let fake = FakeDsm::start().await;
    let gid = engine(&fake, request(&fake, Some(GID)))
        .start()
        .await
        .expect("start");
    assert_eq!(gid, GID);
    assert_eq!(fake.task_calls().await, vec![TASK_GET.to_owned()]);
}

#[tokio::test]
async fn a_new_task_is_created_and_its_id_returned() {
    let fake = FakeDsm::start().await;
    fake.get.always(Reply::Fail(544));
    let gid = engine(&fake, request(&fake, Some(GID)))
        .start()
        .await
        .expect("start");
    assert_eq!(gid, GID);
    assert_eq!(
        fake.task_calls().await,
        vec![
            TASK_GET.to_owned(),
            TASK_LIST.to_owned(),
            TASK_CREATE.to_owned()
        ]
    );
}

#[tokio::test]
async fn the_destination_folder_is_created_before_the_task() {
    let fake = FakeDsm::start().await;
    engine(&fake, request(&fake, None))
        .start()
        .await
        .expect("start");
    let calls = fake.calls().await;
    let names: Vec<&str> = calls.iter().map(|(name, _)| name.as_str()).collect();
    let folder = names
        .iter()
        .position(|name| *name == FOLDER_CREATE)
        .expect("folder");
    let task = names
        .iter()
        .position(|name| *name == TASK_CREATE)
        .expect("task");
    assert!(folder < task, "{names:?}");
    let param = |index: usize, key: &str| {
        calls[index]
            .1
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        param(folder, "folder_path").as_deref(),
        Some(r#"["/Media/Downloads/Torrents"]"#)
    );
    assert_eq!(param(folder, "name").as_deref(), Some(r#"["MyTorrent"]"#));
    assert_eq!(param(folder, "force_parent").as_deref(), Some("true"));
    assert_eq!(
        param(task, "destination").as_deref(),
        Some(r#""Media/Downloads/Torrents/MyTorrent""#)
    );
}

#[tokio::test]
async fn a_session_error_signs_in_again_and_retries() {
    let fake = FakeDsm::start().await;
    fake.folder.then(Reply::Fail(119));
    let gid = engine(&fake, request(&fake, None))
        .start()
        .await
        .expect("start");
    assert_eq!(gid, GID);
    assert_eq!(fake.count("entry.cgi login").await, 2);
    assert_eq!(fake.count(FOLDER_CREATE).await, 2);
}

#[tokio::test]
async fn creating_gives_up_after_five_attempts() {
    let fake = FakeDsm::start().await;
    fake.get.always(Reply::Fail(544));
    fake.create.always(Reply::Fail(401));
    let engine = engine(&fake, request(&fake, Some(GID)));
    let error = engine.start().await.expect_err("never created");
    assert_eq!(error.message(), "Unable to download file");
    assert_eq!(fake.count(TASK_GET).await, 1);
    assert_eq!(fake.count(TASK_LIST).await, 5);
    assert_eq!(fake.count(TASK_CREATE).await, 5);
}

#[tokio::test]
async fn creating_succeeds_on_the_fifth_attempt() {
    let fake = FakeDsm::start().await;
    fake.get.always(Reply::Fail(544));
    fake.create.always(Reply::Data(json!({"task_id": [GID]})));
    for _ in 0..4 {
        fake.create.then(Reply::Fail(401));
    }
    let gid = engine(&fake, request(&fake, Some(GID)))
        .start()
        .await
        .expect("start");
    assert_eq!(gid, GID);
    assert_eq!(fake.count(TASK_GET).await, 1);
    assert_eq!(fake.count(TASK_LIST).await, 5);
    assert_eq!(fake.count(TASK_CREATE).await, 5);
}

#[tokio::test]
async fn a_task_without_id_is_found_by_its_link() {
    let fake = FakeDsm::start().await;
    fake.create.always(Reply::Data(json!({})));
    fake.list.then(tasks(&[])).then(tasks(&[task(2, "")]));
    let gid = engine(&fake, request(&fake, None))
        .start()
        .await
        .expect("start");
    assert_eq!(gid, GID);
    assert_eq!(fake.count(TASK_CREATE).await, 1);
}

#[tokio::test]
async fn a_missing_url_or_account_fails_to_start() {
    let fake = FakeDsm::start().await;
    let no_url = DownloadStationConfig {
        url: None,
        ..config(&fake)
    };
    let error = engine_with(request(&fake, None), no_url)
        .start()
        .await
        .expect_err("no url");
    assert_eq!(
        error.message(),
        "No URL specified for Synology download station"
    );
    let no_password = DownloadStationConfig {
        password: None,
        ..config(&fake)
    };
    let error = engine_with(request(&fake, None), no_password)
        .start()
        .await
        .expect_err("no password");
    assert_eq!(
        error.message(),
        "No username/password specified for Synology download station"
    );
}

#[tokio::test]
async fn a_refused_login_explains_two_factor_accounts() {
    let fake = FakeDsm::start().await;
    let blank_password = DownloadStationConfig {
        password: Some(" ".to_owned()),
        ..config(&fake)
    };
    let error = engine_with(request(&fake, None), blank_password)
        .start()
        .await
        .expect_err("login refused");
    assert_eq!(
        error.message(),
        "DownloadStation login failed for user 'user': the Synology password is required. If \
         2-step verification is enabled on this Synology account, use a dedicated service \
         account without 2FA."
    );
}

#[tokio::test]
async fn the_root_becomes_the_default_destination_when_the_account_has_none() {
    let fake = FakeDsm::start().await;
    fake.config.always(Reply::Data(
        json!({"default_destination": "", "bt_max_download": 5}),
    ));
    engine(&fake, request(&fake, None))
        .start()
        .await
        .expect("start");
    let calls = fake.calls().await;
    let (_, query) = calls
        .iter()
        .find(|(name, _)| name == "DownloadStation/info.cgi setserverconfig")
        .expect("default destination set");
    assert!(query.contains(&(
        "default_destination".to_owned(),
        "/Media/Downloads/Torrents".to_owned()
    )));
    assert!(query.contains(&("bt_max_download".to_owned(), "5".to_owned())));
}

#[tokio::test]
async fn an_existing_default_destination_is_kept() {
    let fake = FakeDsm::start().await;
    engine(&fake, request(&fake, None))
        .start()
        .await
        .expect("start");
    assert_eq!(
        fake.count("DownloadStation/info.cgi setserverconfig").await,
        0
    );
}

#[tokio::test]
async fn without_a_root_the_default_destination_is_used() {
    let fake = FakeDsm::start().await;
    let config = DownloadStationConfig {
        download_path: None,
        ..config(&fake)
    };
    let request = DownloadStationRequest {
        category: Some("movies".to_owned()),
        ..request(&fake, None)
    };
    engine_with(request, config).start().await.expect("start");
    let calls = fake.calls().await;
    let (_, query) = calls
        .iter()
        .find(|(name, _)| name == TASK_CREATE)
        .expect("task created");
    assert!(query.contains(&(
        "destination".to_owned(),
        r#""Media/movies/MyTorrent""#.to_owned()
    )));
}

#[tokio::test]
async fn cancel_deletes_the_task() {
    let fake = FakeDsm::start().await;
    let engine = engine(&fake, request(&fake, Some(GID)));
    engine.start().await.expect("start");
    engine.cancel().await;
    assert_eq!(
        engine.completion().outcome(),
        Some(Err(CANCELLED_MESSAGE.to_owned()))
    );
    assert_eq!(fake.count("DownloadStation/task.cgi delete").await, 1);
}

#[tokio::test]
async fn cancel_before_start_completes_without_calls() {
    let fake = FakeDsm::start().await;
    let engine = engine(&fake, request(&fake, Some(GID)));
    engine.cancel().await;
    assert_eq!(
        engine.completion().outcome(),
        Some(Err(CANCELLED_MESSAGE.to_owned()))
    );
    assert_eq!(fake.calls().await.len(), 0);
}

#[tokio::test]
async fn pause_and_resume_reach_download_station() {
    let fake = FakeDsm::start().await;
    let engine = engine(&fake, request(&fake, Some(GID)));
    engine.start().await.expect("start");
    engine.pause().await;
    engine.resume().await;
    assert_eq!(fake.count("DownloadStation/task.cgi pause").await, 1);
    assert_eq!(fake.count("DownloadStation/task.cgi resume").await, 1);
}

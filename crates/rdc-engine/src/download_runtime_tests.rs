use super::*;
use crate::testing::{
    FakeDebridProvider, TestEngine,
    downloads::{FakeDownloadEngine, FakeDownloadFactory},
};
use rdc_model::settings::Settings;
use std::time::Duration;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{any, method},
};

fn metadata(client: DownloadClient) -> (Torrent, Download) {
    let download = Download {
        download_id: uuid::Uuid::new_v4(),
        link: Some("https://example.invalid/file.bin".to_owned()),
        file_name: Some("file.bin".to_owned()),
        ..Download::default()
    };
    let torrent = Torrent {
        rd_name: Some("Example".to_owned()),
        download_client: client,
        rd_size: Some(123),
        downloads: vec![download.clone()],
        ..Torrent::default()
    };
    (torrent, download)
}

#[tokio::test]
async fn start_failures_and_missing_ids_are_terminal_even_when_backend_does_not_complete() {
    for result in [Err("cannot start".to_owned()), Ok(String::new())] {
        let backend = FakeDownloadEngine::new();
        backend.set_start_result(result);
        let transfer = Transfer::new(Arc::new(backend.clone()), PathBuf::new(), None, 0, false);
        assert!(transfer.start().await.is_err());
        assert!(transfer.completion().is_finished());
        assert!(transfer.completion().wait().await.is_err());
        assert_eq!(backend.cancelled(), 1);
    }
}

#[tokio::test]
async fn unknown_total_uses_provider_size_and_control_calls_reach_the_backend() {
    let backend = FakeDownloadEngine::new();
    backend.set_progress(Progress {
        bytes_done: 10,
        bytes_total: 0,
        speed: 4,
    });
    let transfer = Transfer::new(Arc::new(backend.clone()), PathBuf::new(), None, 123, false);
    assert_eq!(transfer.progress().borrow().bytes_total, 123);
    transfer.start().await.expect("start");
    transfer.pause().await;
    transfer.resume().await;
    assert_eq!(backend.paused(), 1);
    assert_eq!(backend.resumed(), 1);
    transfer.cancel().await;
    assert_eq!(transfer.progress().borrow().speed, 0);
    assert!(transfer.completion().wait().await.is_err());
}

#[tokio::test]
async fn successful_internal_completion_checks_the_expected_file_size() {
    let dir = tempfile::tempdir().expect("directory");
    let output = dir.path().join("file.bin");
    fs::write(&output, b"short").await.expect("file");
    let backend = FakeDownloadEngine::new();
    let transfer = Transfer::new(Arc::new(backend.clone()), output, Some(1_000_000), 0, false);
    transfer.start().await.expect("start");
    backend.complete(Ok(()));
    let error = transfer.completion().wait().await.expect_err("wrong size");
    assert!(error.contains("outside the expected range"));
}

/// A remote job whose creation ignores cancellation, like an aria2 `addUri` call in flight:
/// it removes itself only if its start runs to the end or it is cancelled after creation.
#[derive(Debug)]
struct RemoteJob {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    created: AtomicBool,
    cancelled: AtomicBool,
    removed: AtomicBool,
    progress: watch::Sender<Progress>,
    completion: CompletionSender,
}

impl RemoteJob {
    fn new() -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            created: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            removed: AtomicBool::new(false),
            progress: watch::Sender::new(Progress::default()),
            completion: completion_channel().0,
        }
    }
}

#[async_trait]
impl DownloadEngine for RemoteJob {
    async fn start(&self) -> Result<String, StartError> {
        self.entered.notify_one();
        self.release.notified().await;
        self.created.store(true, Ordering::SeqCst);
        if self.cancelled.load(Ordering::SeqCst) {
            self.removed.store(true, Ordering::SeqCst);
            return Err(StartError::new(rdc_download::CANCELLED_MESSAGE));
        }
        Ok("gid".to_owned())
    }
    fn progress(&self) -> watch::Receiver<Progress> {
        self.progress.subscribe()
    }
    fn completion(&self) -> Completion {
        self.completion.subscribe()
    }
    async fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if self.created.load(Ordering::SeqCst) {
            self.removed.store(true, Ordering::SeqCst);
        }
    }
    async fn pause(&self) {}
    async fn resume(&self) {}
}

#[tokio::test]
async fn cancelling_during_start_lets_the_backend_remove_the_job_it_creates() {
    let backend = Arc::new(RemoteJob::new());
    let transfer = Arc::new(Transfer::new(
        backend.clone(),
        PathBuf::new(),
        None,
        0,
        false,
    ));
    let start = tokio::spawn({
        let transfer = transfer.clone();
        async move { transfer.start().await }
    });
    backend.entered.notified().await;
    transfer.cancel().await;
    backend.release.notify_one();
    let started = tokio::time::timeout(Duration::from_secs(2), start)
        .await
        .expect("start finishes")
        .expect("start task");
    assert!(started.is_err());
    assert!(backend.created.load(Ordering::SeqCst));
    assert!(
        backend.removed.load(Ordering::SeqCst),
        "the created job is removed"
    );
    assert!(transfer.completion().wait().await.is_err());
}

#[tokio::test]
async fn remote_adoption_keeps_the_existing_target_but_fresh_remote_starts_replace_it() {
    let fake = FakeDebridProvider::default();
    let factory = FakeDownloadFactory::new();
    let mut options = fake.options();
    options.download_factory = Some(Arc::new(factory.clone()));
    let test = TestEngine::with_options(Settings::default(), fake, options)
        .await
        .expect("engine");
    for client in [DownloadClient::Aria2c, DownloadClient::DownloadStation] {
        let (torrent, mut download) = metadata(client);
        download.remote_id = Some("adopt-me".to_owned());
        let (_, output, _) = test
            .engine
            .output_paths(&torrent, &download)
            .await
            .expect("paths");
        fs::write(&output, b"keep").await.expect("existing target");
        let transfer = test
            .engine
            .prepare_download(&torrent, &download)
            .await
            .expect("transfer");
        transfer.start().await.expect("start");
        assert_eq!(fs::read(&output).await.expect("target"), b"keep");
        assert_eq!(
            factory
                .requests()
                .last()
                .expect("request")
                .download
                .remote_id
                .as_deref(),
            Some("adopt-me")
        );
        transfer.cancel().await;
    }
    for client in [DownloadClient::Aria2c, DownloadClient::DownloadStation] {
        let (torrent, download) = metadata(client);
        let (_, output, _) = test
            .engine
            .output_paths(&torrent, &download)
            .await
            .expect("paths");
        fs::write(&output, b"stale").await.expect("stale target");
        let transfer = test
            .engine
            .prepare_download(&torrent, &download)
            .await
            .expect("transfer");
        transfer.start().await.expect("start");
        assert!(!output.exists());
        transfer.cancel().await;
    }
    test.engine.shutdown().await;
}

#[tokio::test]
async fn configured_internal_engine_downloads_without_a_custom_factory() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello"))
        .mount(&server)
        .await;
    let test = TestEngine::new().await.expect("engine");
    let (torrent, mut download) = metadata(DownloadClient::Internal);
    download.link = Some(format!("{}/file.bin", server.uri()));
    let (_, output, _) = test
        .engine
        .output_paths(&torrent, &download)
        .await
        .expect("path");
    let transfer = test
        .engine
        .prepare_download(&torrent, &download)
        .await
        .expect("prepare");
    transfer.start().await.expect("start");
    tokio::time::timeout(Duration::from_secs(5), transfer.completion().wait())
        .await
        .expect("timeout")
        .expect("download");
    assert_eq!(fs::read(output).await.expect("file"), b"hello");
    test.engine.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn configured_symlink_engine_links_extensionless_files_using_provider_layout() {
    let test = TestEngine::new().await.expect("engine");
    let mount = test.tempdir.path().join("mount");
    fs::create_dir_all(mount.join("Example"))
        .await
        .expect("mount");
    fs::write(mount.join("Example/entry"), b"mounted")
        .await
        .expect("source");
    test.engine
        .update_setting("DownloadClient:RcloneMountPath", mount.to_str())
        .await
        .expect("mount settings");
    let (mut torrent, mut download) = metadata(DownloadClient::Symlink);
    torrent.client_kind = Some(Provider::AllDebrid);
    download.file_name = Some("entry".to_owned());
    let (_, output, _) = test
        .engine
        .output_paths(&torrent, &download)
        .await
        .expect("path");
    let transfer = test
        .engine
        .prepare_download(&torrent, &download)
        .await
        .expect("prepare");
    transfer.start().await.expect("link");
    transfer.completion().wait().await.expect("completion");
    assert!(
        fs::symlink_metadata(&output)
            .await
            .expect("metadata")
            .is_symlink()
    );
    assert_eq!(fs::read(&output).await.expect("content"), b"mounted");
    test.engine.shutdown().await;
}

#[tokio::test]
async fn configured_aria2_engine_adopts_existing_gid_without_removing_the_local_file() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
        let rpc: serde_json::Value = serde_json::from_slice(&request.body).expect("RPC");
        let result = if rpc["method"] == "aria2.tellStatus" { serde_json::json!({"gid":"adopt","status":"active","totalLength":"4","completedLength":"0","downloadSpeed":"0","files":[]}) } else { serde_json::json!("OK") };
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":rpc["id"],"result":result}))
    }).mount(&server).await;
    let test = TestEngine::new().await.expect("engine");
    test.engine
        .update_setting(
            "DownloadClient:Aria2cUrl",
            Some(&format!("{}/jsonrpc", server.uri())),
        )
        .await
        .expect("aria2 URL");
    let (torrent, mut download) = metadata(DownloadClient::Aria2c);
    download.remote_id = Some("adopt".to_owned());
    let (_, output, _) = test
        .engine
        .output_paths(&torrent, &download)
        .await
        .expect("path");
    fs::write(&output, b"keep").await.expect("file");
    let transfer = test
        .engine
        .prepare_download(&torrent, &download)
        .await
        .expect("prepare");
    assert_eq!(transfer.start().await.expect("adopt"), "adopt");
    assert_eq!(fs::read(output).await.expect("retained"), b"keep");
    transfer.cancel().await;
    test.engine.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn output_paths_sanitize_names_and_refuse_parent_symlinks() {
    let test = TestEngine::new().await.expect("engine");
    let (mut torrent, download) = metadata(DownloadClient::Internal);
    torrent.category = Some("../outside".to_owned());
    torrent.rd_name = Some("../Example".to_owned());
    let (destination, output, _) = test
        .engine
        .output_paths(&torrent, &download)
        .await
        .expect("sanitized");
    assert_eq!(destination.file_name().expect("category"), "outside");
    assert_eq!(
        output.parent().expect("torrent").file_name().expect("name"),
        "Example"
    );
    let outside = test.tempdir.path().join("elsewhere");
    fs::create_dir(&outside).await.expect("outside");
    let root = PathBuf::from(&test.engine.settings().download_client.download_path);
    std::os::unix::fs::symlink(&outside, root.join("linked")).expect("symlink");
    torrent.category = Some("linked".to_owned());
    assert!(test.engine.output_paths(&torrent, &download).await.is_err());
    test.engine.shutdown().await;
}

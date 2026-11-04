//! Behaviour of the internal HTTP engine against a local scripted server.

pub mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rdc_download::{
    CANCELLED_MESSAGE, DownloadEngine, HttpDownload, InternalConfig, Outcome, Progress,
};
use support::{FileOptions, Reply, TestServer, test_data};
use tokio::sync::watch;

const WAIT_LIMIT: Duration = Duration::from_secs(15);

fn config() -> InternalConfig {
    InternalConfig::new()
}

struct Run {
    download: HttpDownload,
    target: PathBuf,
    _dir: tempfile::TempDir,
}

impl Run {
    async fn start(url: &str, config: watch::Receiver<InternalConfig>) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("file.bin");
        let download = HttpDownload::new(url, &target, config);
        let id = download.start().await.expect("start");
        assert_eq!(id.len(), 36, "remote id {id} is a UUID");
        Self {
            download,
            target,
            _dir: dir,
        }
    }

    async fn finish(&self) -> Outcome {
        tokio::time::timeout(WAIT_LIMIT, self.download.completion().wait())
            .await
            .expect("download finishes in time")
    }

    fn partial(&self) -> PathBuf {
        partial_of(&self.target)
    }
}

fn partial_of(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".download");
    PathBuf::from(name)
}

async fn download(url: &str, config: InternalConfig) -> (Run, Outcome) {
    let (_tx, rx) = watch::channel(config);
    let run = Run::start(url, rx).await;
    let outcome = run.finish().await;
    (run, outcome)
}

fn assert_downloaded(run: &Run, data: &[u8]) {
    let written = std::fs::read(&run.target).expect("target file");
    assert!(
        written == data,
        "content differs: {} vs {} bytes",
        written.len(),
        data.len()
    );
    assert!(!run.partial().exists(), "partial file removed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn downloads_a_file_and_reports_its_size() {
    let data = test_data(64 * 1024);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let (run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    assert_eq!(
        *run.download.progress().borrow(),
        Progress {
            bytes_done: 65_536,
            bytes_total: 65_536,
            speed: 0
        }
    );
}

#[tokio::test]
async fn invalid_links_fail_to_start() {
    let (_tx, rx) = watch::channel(config());
    let download = HttpDownload::new("not a url", "/tmp/never", rx);
    let error = download.start().await.expect_err("invalid link");
    assert_eq!(error.message(), "Invalid download link not a url");
    assert_eq!(
        download.completion().outcome(),
        Some(Err("Invalid download link not a url".to_owned()))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_errors_fail_with_the_status_and_remove_the_stale_partial() {
    let server = TestServer::start(|_| Reply::status(404)).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("file.bin");
    std::fs::write(partial_of(&target), b"stale").expect("stale partial");
    let (_tx, rx) = watch::channel(config());
    let download = HttpDownload::new(server.url("/missing"), &target, rx);
    download.start().await.expect("start");
    let outcome = tokio::time::timeout(WAIT_LIMIT, download.completion().wait())
        .await
        .expect("finishes");
    assert_eq!(
        outcome,
        Err("Response status code does not indicate success: 404 (Not Found).".to_owned())
    );
    assert!(!partial_of(&target).exists());
    assert!(!target.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_file_is_replaced_and_stale_partial_removed() {
    let data = test_data(4096);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("file.bin");
    std::fs::write(&target, b"old content").expect("old file");
    std::fs::write(partial_of(&target), vec![7; 100_000]).expect("stale partial");
    let (_tx, rx) = watch::channel(config());
    let download = HttpDownload::new(server.url("/file.bin"), &target, rx);
    download.start().await.expect("start");
    assert_eq!(download.completion().wait().await, Ok(()));
    assert_eq!(std::fs::read(&target).expect("target"), *data);
    assert!(!partial_of(&target).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_identify_the_client_and_ask_for_identity_encoding() {
    let data = test_data(2048);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let (_run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert_eq!(outcome, Ok(()));
    for request in server.requests() {
        assert_eq!(
            request.header("user-agent"),
            Some(concat!("rd-client/", env!("CARGO_PKG_VERSION")))
        );
        assert_eq!(request.header("accept"), Some("*/*"));
        assert_eq!(request.header("accept-encoding"), Some("identity"));
    }
}

#[tokio::test]
async fn cancel_before_start_completes_and_start_then_fails() {
    let (_tx, rx) = watch::channel(config());
    let download = HttpDownload::new("http://127.0.0.1:9/file", "/nonexistent/file", rx);
    download.cancel().await;
    assert_eq!(
        download.completion().outcome(),
        Some(Err(CANCELLED_MESSAGE.to_owned()))
    );
    let error = download.start().await.expect_err("cancelled");
    assert_eq!(error.message(), CANCELLED_MESSAGE);
}

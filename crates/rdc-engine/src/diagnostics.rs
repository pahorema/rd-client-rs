//! Explicit filesystem, downloader and RPC diagnostics.

use crate::{Engine, EngineError};
use rdc_download::DownloadEngine;
use rdc_model::{Download, DownloadClient, Torrent};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{fs, io::AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Public test file (reachable over IPv4 and IPv6) for providers without their own speed test.
const FALLBACK_SPEED_TEST_URL: &str = "https://speed.cloudflare.com/__down?bytes=52428800";
/// Stop the speed probe after transferring fifty mebibytes.
const SPEED_TEST_BYTES: u64 = 50 * 1024 * 1024;
/// Filesystem throughput probe size.
const WRITE_TEST_BYTES: u64 = 64 * 1024 * 1024;

struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Engine {
    /// Checks that an existing directory accepts a newly created file and deletes the probe.
    ///
    /// Uses an exclusive randomized filename so existing user files are never overwritten.
    pub async fn test_path(&self, path: &str) -> Result<(), EngineError> {
        if path.trim().is_empty() {
            return Err(EngineError::Message("Invalid path".to_owned()));
        }
        let path = Path::new(path);
        if !path.is_dir() {
            return Err(EngineError::Message(format!(
                "Path {} does not exist",
                path.display()
            )));
        }
        let filename = probe_path(path, "txt");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&filename)
            .await?;
        let result = file
            .write_all(b"rd-client Test File, you can remove this file.")
            .await;
        drop(file);
        let cleanup = fs::remove_file(&filename).await;
        result?;
        cleanup?;
        Ok(())
    }

    /// Measures writing sixty-four mebibytes to the configured download directory, in bytes/s.
    ///
    /// The probe is removed on success, error, request cancellation and engine shutdown.
    pub async fn test_write_speed(&self) -> Result<f64, EngineError> {
        let path = PathBuf::from(&self.settings().download_client.download_path);
        let cancel = self.inner.cancel.child_token();
        let _cancel_on_drop = CancelOnDrop(cancel.clone());
        tokio::spawn(async move { write_speed(&path, &cancel).await })
            .await
            .map_err(|error| EngineError::Message(error.to_string()))?
    }

    /// Measures the configured downloader against the provider's speed-test file (or a public
    /// one when the provider has none), in bytes/s.
    ///
    /// Symlink mode uses the internal downloader. The probe stops after fifty mebibytes
    /// or completion; request cancellation and shutdown stop it and remove its local file.
    pub async fn test_download_speed(&self) -> Result<u64, EngineError> {
        let engine = self.clone();
        let cancel = self.inner.cancel.child_token();
        let _cancel_on_drop = CancelOnDrop(cancel.clone());
        tokio::spawn(async move { engine.download_speed(&cancel).await })
            .await
            .map_err(|error| EngineError::Message(error.to_string()))?
    }

    /// Calls aria2's version RPC using the supplied URL and optional secret.
    pub async fn test_aria2(
        &self,
        url: &str,
        secret: Option<&str>,
    ) -> Result<rdc_aria2::Version, EngineError> {
        if url.trim().is_empty() {
            return Err(EngineError::Message("Invalid Url".to_owned()));
        }
        let mut builder = rdc_aria2::Client::builder(url).timeout(Duration::from_secs(10));
        if let Some(secret) = secret {
            builder = builder.secret(secret);
        }
        let client = builder
            .build()
            .map_err(|error| EngineError::Message(error.to_string()))?;
        client
            .version()
            .await
            .map_err(|error| EngineError::Message(error.to_string()))
    }

    async fn download_speed(&self, cancel: &CancellationToken) -> Result<u64, EngineError> {
        let settings = self.settings();
        let output = probe_path(Path::new(&settings.download_client.download_path), "rar");
        let mut torrent = Torrent {
            download_client: settings.download_client.client,
            ..Torrent::default()
        };
        if torrent.download_client == DownloadClient::Symlink {
            torrent.download_client = DownloadClient::Internal;
        }
        let url = match self.provider().speed_test_url().await {
            Ok(Some(url)) => url,
            Ok(None) => FALLBACK_SPEED_TEST_URL.to_owned(),
            Err(error) => {
                tracing::warn!(%error, "Unable to get the provider speed test, using the public one");
                FALLBACK_SPEED_TEST_URL.to_owned()
            }
        };
        let download = Download {
            link: Some(url),
            ..Download::default()
        };
        let relative = output
            .file_name()
            .ok_or_else(|| EngineError::Message("Invalid download path".to_owned()))?
            .to_string_lossy()
            .into_owned();
        let transfer = tokio::select! {
            () = cancel.cancelled() => return Err(EngineError::Message("The download was cancelled".to_owned())),
            result = self.configured_download(&torrent, &download, output.clone(), relative) => result?,
        };
        let result = run_speed_probe(&transfer, cancel).await;
        transfer.cancel().await;
        let cleanup = fs::remove_file(&output).await;
        if let Err(error) = cleanup
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.into());
        }
        result
    }
}

fn probe_path(directory: &Path, extension: &str) -> PathBuf {
    directory.join(format!("rd-client-test-{}.{extension}", Uuid::new_v4()))
}

async fn run_speed_probe(
    transfer: &Arc<dyn DownloadEngine>,
    cancel: &CancellationToken,
) -> Result<u64, EngineError> {
    let id = tokio::select! {
        () = cancel.cancelled() => return Err(EngineError::Message("The download was cancelled".to_owned())),
        result = transfer.start() => result.map_err(|error| EngineError::Message(error.to_string()))?,
    };
    if id.trim().is_empty() {
        return Err(EngineError::Message(
            "No remote ID received from download client".to_owned(),
        ));
    }
    let mut completion = transfer.completion();
    let mut progress = transfer.progress();
    let mut progress_open = true;
    loop {
        let current = *progress.borrow_and_update();
        let speed = current.speed;
        if current.bytes_done > SPEED_TEST_BYTES {
            return Ok(speed);
        }
        tokio::select! {
            () = cancel.cancelled() => return Err(EngineError::Message("The download was cancelled".to_owned())),
            outcome = completion.wait() => return outcome.map(|()| speed).map_err(EngineError::Message),
            changed = progress.changed(), if progress_open => {
                progress_open = changed.is_ok();
            }
        }
    }
}

async fn write_speed(directory: &Path, cancel: &CancellationToken) -> Result<f64, EngineError> {
    let output = probe_path(directory, "tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .await?;
    let start = Instant::now();
    let mut buffer = vec![0_u8; 64 * 1024];
    fastrand::fill(&mut buffer);
    let result = async {
        for _ in 0..(WRITE_TEST_BYTES / 65_536) {
            tokio::select! {
                () = cancel.cancelled() => return Err(EngineError::Message("Write speed test cancelled".to_owned())),
                result = file.write_all(&buffer) => result?,
            }
        }
        file.sync_data().await?;
        Ok::<_, EngineError>(())
    }.await;
    drop(file);
    let cleanup = fs::remove_file(&output).await;
    result?;
    cleanup?;
    Ok(64.0 * 1024.0 * 1024.0 / start.elapsed().as_secs_f64().max(f64::MIN_POSITIVE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DownloadFactory, DownloadRequest,
        test_download::TestDownload,
        testing::{FakeDebridProvider, TestEngine},
    };
    use async_trait::async_trait;
    use rdc_download::Progress;
    use rdc_model::settings::Settings;
    use std::sync::{Mutex, atomic::Ordering};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    #[derive(Debug)]
    struct ProbeFactory {
        transfer: Arc<TestDownload>,
        requests: Mutex<Vec<DownloadRequest>>,
    }

    #[async_trait]
    impl DownloadFactory for ProbeFactory {
        async fn create(
            &self,
            request: DownloadRequest,
        ) -> Result<Arc<dyn DownloadEngine>, EngineError> {
            self.requests.lock().expect("requests").push(request);
            Ok(self.transfer.clone())
        }
    }

    #[tokio::test]
    async fn path_probe_preserves_existing_test_file_and_removes_its_own_file() {
        let test = TestEngine::new().await.expect("engine");
        let directory = PathBuf::from(&test.engine.settings().download_client.download_path);
        fs::write(directory.join("test.txt"), b"keep")
            .await
            .expect("existing file");
        test.engine
            .test_path(directory.to_str().expect("path"))
            .await
            .expect("path check");
        assert_eq!(
            fs::read(directory.join("test.txt"))
                .await
                .expect("existing"),
            b"keep"
        );
        let mut entries = fs::read_dir(&directory).await.expect("entries");
        assert_eq!(
            entries
                .next_entry()
                .await
                .expect("entry")
                .expect("test.txt")
                .file_name(),
            "test.txt"
        );
        assert!(entries.next_entry().await.expect("entry").is_none());
        assert_eq!(
            test.engine
                .test_path("")
                .await
                .expect_err("empty")
                .to_string(),
            "Invalid path"
        );
        assert!(
            test.engine
                .test_path("/not-a-real-rd-client-path")
                .await
                .is_err()
        );
        test.engine.shutdown().await;
    }

    #[tokio::test]
    async fn write_probe_reports_throughput_and_cleans_temporary_file() {
        let test = TestEngine::new().await.expect("engine");
        assert!(test.engine.test_write_speed().await.expect("speed") > 0.0);
        let mut entries = fs::read_dir(&test.engine.settings().download_client.download_path)
            .await
            .expect("entries");
        assert!(entries.next_entry().await.expect("entry").is_none());
        test.engine.shutdown().await;
    }

    #[tokio::test]
    async fn download_probe_uses_injected_factory_and_internal_mode_for_symlinks() {
        let fake = FakeDebridProvider::default();
        let transfer = Arc::new(TestDownload::new(false));
        transfer.progress.send_replace(Progress {
            bytes_done: SPEED_TEST_BYTES + 1,
            bytes_total: 90_000_000,
            speed: 1234,
        });
        let factory = Arc::new(ProbeFactory {
            transfer: transfer.clone(),
            requests: Mutex::new(Vec::new()),
        });
        let mut options = fake.options();
        options.download_factory = Some(factory.clone());
        let mut settings = Settings::default();
        settings.download_client.client = DownloadClient::Symlink;
        let test = TestEngine::with_options(settings, fake, options)
            .await
            .expect("engine");
        assert_eq!(
            test.engine.test_download_speed().await.expect("speed"),
            1234
        );
        assert_eq!(transfer.cancelled.load(Ordering::Relaxed), 1);
        {
            let requests = factory.requests.lock().expect("requests");
            assert_eq!(requests.len(), 1);
            assert_eq!(
                requests[0].torrent.download_client,
                DownloadClient::Internal
            );
        }
        test.engine.shutdown().await;
    }

    #[tokio::test]
    async fn aria2_probe_returns_real_rpc_version_without_network_services() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": "1", "result": {"version": "1.37.0", "enabledFeatures": ["BitTorrent"]}
        }))).mount(&server).await;
        let test = TestEngine::new().await.expect("engine");
        let version = test
            .engine
            .test_aria2(&format!("{}/jsonrpc", server.uri()), None)
            .await
            .expect("version");
        assert_eq!(version.version, "1.37.0");
        assert_eq!(
            test.engine
                .test_aria2("", None)
                .await
                .expect_err("invalid URL")
                .to_string(),
            "Invalid Url"
        );
        test.engine.shutdown().await;
    }
}

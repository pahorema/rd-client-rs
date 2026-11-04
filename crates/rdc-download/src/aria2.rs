//! The aria2 engine: hands a download to an aria2 daemon and follows it over JSON-RPC.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use rdc_aria2::{AddUriOptions, Client, DownloadState, DownloadStatus, Error};
use tokio::sync::watch;

use crate::engine::{CANCELLED_MESSAGE, Completion, DownloadEngine, Outcome, Progress, StartError};
use crate::remote::{Polled, Signals, Timing, remote_parent, remote_path, spawn_polling};

/// Timeout of each aria2 request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Transport retries of the requests that start a download.
const START_TRANSPORT_RETRIES: u32 = 10;
/// Transport retries of the requests that follow, pause or remove a download.
const FOLLOW_TRANSPORT_RETRIES: u32 = 1;
/// Retries of the add-and-verify sequence before the start fails.
const START_RETRIES: u32 = 5;

/// Where the aria2 daemon is and where it saves files.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Aria2Config {
    /// JSON-RPC URL, ending with `/jsonrpc`.
    pub url: String,
    /// RPC secret; blank means aria2 runs without one.
    pub secret: Option<String>,
    /// Root folder on the aria2 host; blank means aria2 sees the same paths as rd-client.
    /// In that case the native local directory is preserved, including Windows separators.
    pub download_path: Option<String>,
}

impl fmt::Debug for Aria2Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Aria2Config")
            .field("url", &self.url)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("download_path", &self.download_path)
            .finish()
    }
}

/// What to download and where.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Aria2Request {
    /// Link aria2 downloads.
    pub url: String,
    /// Local path of the finished file; only checked for existence, never written.
    pub file_path: PathBuf,
    /// Path of the file below the download folder (`<torrent>/<sub dirs>/<file>`).
    pub relative_path: String,
    /// Category folder between the root and the relative path.
    pub category: Option<String>,
    /// GID of an earlier start; an aria2 download with this GID is adopted.
    pub gid: Option<String>,
}

/// Downloads a file with aria2.
///
/// [`start`](DownloadEngine::start) adopts the download of [`Aria2Request::gid`] when aria2
/// still knows it, otherwise refuses a link aria2 already downloads and adds it (up to six
/// tries). It returns the GID. The engine then asks aria2 about the download every second:
/// an error removes it and fails, a completed or removed download is removed and succeeds as
/// soon as the local file exists (looked for ten times, about 45 s), and an unknown GID fails.
/// Pausing and resuming pause and unpause the aria2 download; cancelling removes it.
#[derive(Debug)]
pub struct Aria2Download {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    request: Aria2Request,
    config: Aria2Config,
    remote_path: String,
    timing: Timing,
    signals: Signals,
    gid: Mutex<Option<String>>,
    /// Client of the requests made after the start.
    client: OnceLock<Client>,
}

impl Aria2Download {
    /// Prepares the download described by `request` on the aria2 daemon of `config`.
    pub fn new(request: Aria2Request, config: Aria2Config) -> Self {
        let remote_path = match config.download_path.as_deref() {
            Some(root) if !root.trim().is_empty() => {
                remote_path(root, request.category.as_deref(), &request.relative_path)
            }
            _ => request.file_path.to_string_lossy().into_owned(),
        };
        Self {
            shared: Arc::new(Shared {
                request,
                config,
                remote_path,
                timing: Timing::default(),
                signals: Signals::new(),
                gid: Mutex::new(None),
                client: OnceLock::new(),
            }),
        }
    }

    /// Replaces the wait between two looks at aria2 (one second by default).
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.shared_mut().timing.poll_interval = interval;
        self
    }

    /// Replaces the wait unit of the retry loops (one second by default): retry `n` of the
    /// start and check `n` for the local file wait `n` units.
    #[must_use]
    pub fn with_retry_step(mut self, step: Duration) -> Self {
        self.shared_mut().timing.retry_step = step;
        self
    }

    fn shared_mut(&mut self) -> &mut Shared {
        Arc::get_mut(&mut self.shared).expect("configured before it is shared")
    }
}

impl Shared {
    fn client(&self, transport_retries: u32) -> Result<Client, Error> {
        let mut builder = Client::builder(self.config.url.clone())
            .timeout(REQUEST_TIMEOUT)
            .retry_count(transport_retries)
            .retry_delay(self.timing.retry_step);
        if let Some(secret) = &self.config.secret {
            builder = builder.secret(secret.clone());
        }
        builder.build()
    }

    fn gid(&self) -> Option<String> {
        self.gid.lock().expect("gid lock poisoned").clone()
    }

    async fn start(&self) -> Result<String, StartError> {
        let fail = |message: String| self.signals.fail(message);
        let client = self
            .client(START_TRANSPORT_RETRIES)
            .map_err(|error| fail(error.to_string()))?;
        let follow = self
            .client(FOLLOW_TRANSPORT_RETRIES)
            .map_err(|error| fail(error.to_string()))?;
        self.client.get_or_init(|| follow);
        let file_path = &self.request.file_path;
        let dir = if self
            .config
            .download_path
            .as_deref()
            .is_some_and(|root| !root.trim().is_empty())
        {
            remote_parent(&self.remote_path).map(std::borrow::Cow::Borrowed)
        } else {
            file_path.parent().map(|parent| parent.to_string_lossy())
        };
        let Some(dir) = dir else {
            return Err(fail(format!("Invalid file path {}", file_path.display())));
        };
        let options = AddUriOptions {
            dir: Some(dir.into_owned()),
            out: file_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
        };
        let url = self.request.url.as_str();
        tracing::debug!(
            "Starting aria2 download of {url} to {} (on aria2 {})",
            file_path.display(),
            self.remote_path
        );
        let mut gid = self
            .request
            .gid
            .clone()
            .filter(|gid| !gid.trim().is_empty());
        if gid.is_none() {
            let downloads = client
                .tell_all()
                .await
                .map_err(|error| fail(error.to_string()))?;
            if downloads.iter().any(|download| has_uri(download, url)) {
                return Err(fail(format!(
                    "The download link {url} has already been added to Aria2"
                )));
            }
        }
        let mut retry = 0;
        let gid = loop {
            match add_or_adopt(&client, &mut gid, url, &options).await {
                Ok(gid) => break gid,
                Err(error) if retry >= START_RETRIES => return Err(fail(error.to_string())),
                Err(error) => {
                    tracing::debug!("Error starting aria2 download: {error}, retry {retry}");
                    if !self.signals.sleep(self.timing.retry_step * retry).await {
                        return Err(StartError::new(CANCELLED_MESSAGE));
                    }
                    retry += 1;
                }
            }
        };
        *self.gid.lock().expect("gid lock poisoned") = Some(gid.clone());
        if self.signals.cancel.is_cancelled() {
            self.remove().await;
            return Err(StartError::new(CANCELLED_MESSAGE));
        }
        Ok(gid)
    }

    /// Removes the download and its result from aria2, ignoring failures.
    async fn remove(&self) {
        let (Some(gid), Some(client)) = (self.gid(), self.client.get()) else {
            return;
        };
        if let Err(error) = client.force_remove(&gid).await {
            tracing::debug!("Ignoring failed aria2 removal of {gid}: {error}");
        }
        if let Err(error) = client.remove_download_result(&gid).await {
            tracing::debug!("Ignoring failed aria2 result removal of {gid}: {error}");
        }
    }

    fn report(&self, status: &DownloadStatus) {
        self.signals.report(Progress {
            bytes_done: status.completed_length,
            bytes_total: status.total_length,
            speed: status.download_speed,
        });
    }
}

/// Adopts `gid` when aria2 knows it, otherwise adds `url` and verifies the new download.
async fn add_or_adopt(
    client: &Client,
    gid: &mut Option<String>,
    url: &str,
    options: &AddUriOptions,
) -> Result<String, Error> {
    if let Some(known) = gid.as_deref() {
        if client.tell_status(known).await.is_ok() {
            return Ok(known.to_owned());
        }
        *gid = None;
    }
    let added = client.add_uri(&[url], options).await?;
    tracing::debug!("Added {url} to aria2 as {added}");
    let added = gid.insert(added);
    client.tell_status(added).await?;
    Ok(added.clone())
}

fn has_uri(download: &DownloadStatus, url: &str) -> bool {
    download
        .files
        .iter()
        .any(|file| file.uris.iter().any(|uri| uri.uri == url))
}

#[async_trait]
impl Polled for Shared {
    fn signals(&self) -> &Signals {
        &self.signals
    }

    async fn poll(&self) -> Option<Outcome> {
        let (gid, client) = (self.gid()?, self.client.get()?);
        let status = match client.tell_status(&gid).await {
            Ok(status) => status,
            Err(Error::Rpc { .. }) => {
                return Some(Err("Download was not found in Aria2".to_owned()));
            }
            Err(error) => {
                tracing::warn!("Unable to get the aria2 status of {gid}: {error}");
                return None;
            }
        };
        let message = status.error_message.as_deref().unwrap_or_default();
        if !message.trim().is_empty() || status.status == DownloadState::Error {
            self.remove().await;
            let code = status.error_code.as_deref().unwrap_or_default();
            return Some(Err(format!("{code}: {message}")));
        }
        self.report(&status);
        if !matches!(
            status.status,
            DownloadState::Complete | DownloadState::Removed
        ) {
            return None;
        }
        tracing::debug!("aria2 download {gid} finished");
        self.remove().await;
        let file_path = &self.request.file_path;
        let found = self
            .signals
            .wait_for_file(file_path, self.timing.retry_step)
            .await?;
        Some(if found {
            Ok(())
        } else {
            Err(format!(
                "File not found at {} (on aria2 {})",
                file_path.display(),
                self.remote_path
            ))
        })
    }
}

#[async_trait]
impl DownloadEngine for Aria2Download {
    async fn start(&self) -> Result<String, StartError> {
        self.shared.signals.begin()?;
        let gid = self.shared.start().await?;
        spawn_polling(Arc::clone(&self.shared), self.shared.timing.poll_interval);
        Ok(gid)
    }

    fn progress(&self) -> watch::Receiver<Progress> {
        self.shared.signals.progress.subscribe()
    }

    fn completion(&self) -> Completion {
        self.shared.signals.completion.subscribe()
    }

    /// Stops following the download and removes it from aria2.
    async fn cancel(&self) {
        self.shared.signals.cancel();
        self.shared.remove().await;
    }

    async fn pause(&self) {
        let (Some(gid), Some(client)) = (self.shared.gid(), self.shared.client.get()) else {
            return;
        };
        if let Err(error) = client.pause(&gid).await {
            tracing::debug!("Ignoring failed aria2 pause of {gid}: {error}");
        }
    }

    async fn resume(&self) {
        let (Some(gid), Some(client)) = (self.shared.gid(), self.shared.client.get()) else {
            return;
        };
        if let Err(error) = client.unpause(&gid).await {
            tracing::debug!("Ignoring failed aria2 unpause of {gid}: {error}");
        }
    }
}

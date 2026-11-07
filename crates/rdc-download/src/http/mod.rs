//! The internal multi-connection HTTP engine.
//!
//! A download first probes the link with a `Range: bytes=0-0` request (following redirects)
//! to learn the file size and whether the server serves byte ranges. It then writes into
//! `<file>.download` — removing a stale one first — split into chunks fetched over up to
//! [`InternalConfig::parallel_connections`] connections, and on success flushes it to disk and
//! renames it to `<file>`, replacing an existing file. On failure or cancellation the partial
//! file is removed.
//!
//! Each read waits at most the configured timeout. When the server serves ranges, a chunk that
//! fails with a passing error resumes from where it stopped, up to five times, after a random
//! exponential backoff of at most 10 s; its read timeout grows after each retry. A
//! multi-connection download that fails before any byte arrived is retried once over a single
//! connection.
//!
//! Ranged requests carry the probe's strong `ETag` (else its `Last-Modified`) as `If-Range`, and
//! their answers must start at the requested offset: a file that changed on the server or a
//! range served from the wrong place fails the download instead of corrupting it.
//!
//! Progress reports bytes written, the expected size (`0` while unknown) and the speed averaged
//! over the last two seconds. An optional speed limit is shared by all connections of a
//! download. Pausing stops reading; connections are closed and reopened at the right offset on
//! resume when the server serves ranges, otherwise they are kept open.

mod config;
mod connection;
mod error;
mod plan;
mod probe;
mod rate;
mod transfer;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Url;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub use self::config::InternalConfig;
use self::connection::Context;
use self::error::TransferError;
use self::rate::SpeedMeter;
use crate::engine::{
    CANCELLED_MESSAGE, Completion, CompletionSender, DownloadEngine, Outcome, Progress,
    STOPPED_UNEXPECTEDLY_MESSAGE, StartError, completion_channel,
};

/// How often progress is published while a download runs.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Downloads one HTTP(S) link to a local file with the internal engine.
///
/// [`start`](DownloadEngine::start) returns a random UUID as remote id and runs the transfer
/// in the background on the current Tokio runtime. Cancelling completes the download with
/// [`CANCELLED_MESSAGE`] after the partial file was removed.
#[derive(Debug)]
pub struct HttpDownload {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    url: String,
    file_path: PathBuf,
    config: watch::Receiver<InternalConfig>,
    progress: watch::Sender<Progress>,
    completion: CompletionSender,
    cancel: CancellationToken,
    paused: watch::Sender<bool>,
    started: AtomicBool,
}

impl HttpDownload {
    /// Prepares a download of `url` to `file_path` using the live `config`.
    pub fn new(
        url: impl Into<String>,
        file_path: impl Into<PathBuf>,
        config: watch::Receiver<InternalConfig>,
    ) -> Self {
        let (completion, _) = completion_channel();
        Self {
            shared: Arc::new(Shared {
                url: url.into(),
                file_path: file_path.into(),
                config,
                progress: watch::Sender::new(Progress::default()),
                completion,
                cancel: CancellationToken::new(),
                paused: watch::Sender::new(false),
                started: AtomicBool::new(false),
            }),
        }
    }

    fn fail_start(&self, message: String) -> StartError {
        self.shared.completion.complete(Err(message.clone()));
        StartError::new(message)
    }
}

#[async_trait]
impl DownloadEngine for HttpDownload {
    async fn start(&self) -> Result<String, StartError> {
        if self.shared.started.swap(true, Ordering::SeqCst) {
            return Err(StartError::new("The download was already started"));
        }
        if self.shared.cancel.is_cancelled() {
            return Err(self.fail_start(CANCELLED_MESSAGE.to_owned()));
        }
        let url = match Url::parse(self.shared.url.trim()) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => url,
            _ => {
                return Err(self.fail_start(format!("Invalid download link {}", self.shared.url)));
            }
        };
        let task = tokio::spawn(run(Arc::clone(&self.shared), url));
        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move {
            if let Err(error) = task.await {
                tracing::error!(
                    "The download of {} stopped unexpectedly: {error}",
                    shared.url
                );
                shared
                    .completion
                    .complete(Err(STOPPED_UNEXPECTEDLY_MESSAGE.to_owned()));
            }
        });
        Ok(uuid::Uuid::new_v4().to_string())
    }

    fn progress(&self) -> watch::Receiver<Progress> {
        self.shared.progress.subscribe()
    }

    fn completion(&self) -> Completion {
        self.shared.completion.subscribe()
    }

    /// Stops the download and returns once its partial file was removed.
    async fn cancel(&self) {
        self.shared.cancel.cancel();
        if !self.shared.started.load(Ordering::SeqCst) {
            self.shared
                .completion
                .complete(Err(CANCELLED_MESSAGE.to_owned()));
        }
        self.completion().wait().await.ok();
    }

    async fn pause(&self) {
        self.shared.paused.send_replace(true);
    }

    async fn resume(&self) {
        self.shared.paused.send_replace(false);
    }
}

/// Runs the whole download and reports its outcome.
async fn run(shared: Arc<Shared>, url: Url) {
    let context = Arc::new(Context::new(
        shared.config.clone(),
        shared.cancel.clone(),
        shared.paused.subscribe(),
    ));
    let partial = transfer::partial_path(&shared.file_path);
    let download = download(
        &context,
        &url,
        &shared.file_path,
        &partial,
        &shared.progress,
    );
    let outcome: Outcome = match download.await {
        Ok(total) => {
            shared.progress.send_replace(Progress {
                bytes_done: total,
                bytes_total: total,
                speed: 0,
            });
            tracing::debug!("Downloaded {url} to {}", shared.file_path.display());
            Ok(())
        }
        Err(error) => {
            if let Err(cleanup) = transfer::remove_file_if_exists(&partial).await {
                tracing::warn!("{cleanup}");
            }
            shared.progress.send_modify(|progress| progress.speed = 0);
            let error = if shared.cancel.is_cancelled() {
                TransferError::Cancelled
            } else {
                error
            };
            tracing::debug!("Download of {url} failed: {error}");
            Err(error.to_string())
        }
    };
    shared.completion.complete(outcome);
}

/// Fetches the file into `partial` while publishing progress and moves it to `target`;
/// returns its size.
async fn download(
    context: &Arc<Context>,
    url: &Url,
    target: &Path,
    partial: &Path,
    progress: &watch::Sender<Progress>,
) -> Result<u64, TransferError> {
    transfer::remove_file_if_exists(partial).await?;
    let remote = probe::probe(context, url).await?;
    let total = remote.size.unwrap_or(0);
    progress.send_replace(Progress {
        bytes_done: 0,
        bytes_total: total,
        speed: 0,
    });
    let fetch = transfer::fetch_file(context, &remote, partial);
    tokio::pin!(fetch);
    let mut meter = SpeedMeter::new();
    let mut ticker = tokio::time::interval(PROGRESS_INTERVAL);
    let size = loop {
        tokio::select! {
            result = &mut fetch => break result?,
            _ = ticker.tick() => {
                let done = context.received();
                progress.send_replace(Progress {
                    bytes_done: done,
                    bytes_total: total,
                    speed: meter.record(Instant::now(), done),
                });
            }
        }
    };
    if context.cancel.is_cancelled() {
        return Err(TransferError::Cancelled);
    }
    transfer::move_into_place(partial, target).await?;
    Ok(size)
}

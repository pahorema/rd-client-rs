//! The internal HTTP engine.

mod config;
mod connection;
mod error;
mod transfer;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use reqwest::Url;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub use self::config::InternalConfig;
use self::connection::Context;
use self::error::TransferError;
use crate::engine::{
    CANCELLED_MESSAGE, Completion, CompletionSender, DownloadEngine, Outcome, Progress, StartError,
    completion_channel,
};

/// Downloads one HTTP(S) link to a local file with the internal engine.
///
/// [`start`](DownloadEngine::start) returns a random UUID as remote id and runs the transfer
/// in the background on the current Tokio runtime. Cancelling completes the download with
/// [`CANCELLED_MESSAGE`].
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
        tokio::spawn(run(Arc::clone(&self.shared), url));
        Ok(uuid::Uuid::new_v4().to_string())
    }

    fn progress(&self) -> watch::Receiver<Progress> {
        self.shared.progress.subscribe()
    }

    fn completion(&self) -> Completion {
        self.shared.completion.subscribe()
    }

    /// Stops the download and returns once it finished.
    async fn cancel(&self) {
        self.shared.cancel.cancel();
        if !self.shared.started.load(Ordering::SeqCst) {
            self.shared
                .completion
                .complete(Err(CANCELLED_MESSAGE.to_owned()));
        }
        self.completion().wait().await.ok();
    }

    async fn pause(&self) {}

    async fn resume(&self) {}
}

/// Runs the whole download and reports its outcome.
async fn run(shared: Arc<Shared>, url: Url) {
    let context = Arc::new(Context::new(shared.config.clone(), shared.cancel.clone()));
    let outcome: Outcome = match download(&context, &url, &shared.file_path).await {
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

/// Fetches the file; returns its size.
async fn download(context: &Arc<Context>, url: &Url, target: &Path) -> Result<u64, TransferError> {
    let size = transfer::fetch_file(context, url, target).await?;
    if context.cancel.is_cancelled() {
        return Err(TransferError::Cancelled);
    }
    Ok(size)
}

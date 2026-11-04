//! Pieces shared by the engines that drive a remote downloader (aria2, Download Station).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::engine::{
    CANCELLED_MESSAGE, CompletionSender, Outcome, Progress, STOPPED_UNEXPECTEDLY_MESSAGE,
    StartError, completion_channel,
};

/// How often a remote engine asks its backend about the transfer.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Default wait unit between retries; retry `n` waits `n` units.
const DEFAULT_RETRY_STEP: Duration = Duration::from_secs(1);
/// How often the local file is looked for after the backend reported completion.
const FILE_CHECKS: u32 = 10;

/// Timing of a remote engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timing {
    /// Wait between two looks at the backend.
    pub(crate) poll_interval: Duration,
    /// Wait unit of the retry loops: retry `n` waits `n` units.
    pub(crate) retry_step: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
            retry_step: DEFAULT_RETRY_STEP,
        }
    }
}

/// Progress, outcome and cancellation of one remote transfer.
#[derive(Debug)]
pub(crate) struct Signals {
    pub(crate) progress: watch::Sender<Progress>,
    pub(crate) completion: CompletionSender,
    pub(crate) cancel: CancellationToken,
    started: AtomicBool,
}

impl Signals {
    pub(crate) fn new() -> Self {
        let (completion, _) = completion_channel();
        Self {
            progress: watch::Sender::new(Progress::default()),
            completion,
            cancel: CancellationToken::new(),
            started: AtomicBool::new(false),
        }
    }

    /// Marks the transfer as started; fails when it already was or was cancelled.
    pub(crate) fn begin(&self) -> Result<(), StartError> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Err(StartError::new("The download was already started"));
        }
        if self.cancel.is_cancelled() {
            return Err(self.fail(CANCELLED_MESSAGE.to_owned()));
        }
        Ok(())
    }

    /// Completes the transfer with `message` and returns it as a start error.
    pub(crate) fn fail(&self, message: String) -> StartError {
        self.completion.complete(Err(message.clone()));
        StartError::new(message)
    }

    /// Stops polling and completes the transfer as cancelled.
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
        self.completion.complete(Err(CANCELLED_MESSAGE.to_owned()));
    }

    pub(crate) fn report(&self, progress: Progress) {
        self.progress.send_replace(progress);
    }

    /// Sleeps for `duration`; `false` when the transfer was cancelled meanwhile.
    pub(crate) async fn sleep(&self, duration: Duration) -> bool {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => false,
            () = tokio::time::sleep(duration) => true,
        }
    }

    /// Looks for the local file up to ten times, check `n` waiting `n` units of `step` after
    /// a miss; `None` when the transfer was cancelled meanwhile.
    pub(crate) async fn wait_for_file(&self, path: &Path, step: Duration) -> Option<bool> {
        for attempt in 0..FILE_CHECKS {
            if tokio::fs::try_exists(path).await.unwrap_or(false) {
                return Some(true);
            }
            if !self.sleep(step.saturating_mul(attempt)).await {
                return None;
            }
        }
        Some(false)
    }
}

/// A remote transfer that is watched by polling its backend.
#[async_trait]
pub(crate) trait Polled: Send + Sync + 'static {
    fn signals(&self) -> &Signals;

    /// One look at the backend; `Some` ends the transfer with that outcome.
    async fn poll(&self) -> Option<Outcome>;
}

/// Polls `transfer` every `interval` until it reports an outcome or is cancelled.
pub(crate) fn spawn_polling(transfer: Arc<impl Polled>, interval: Duration) {
    let watched = Arc::clone(&transfer);
    let task = tokio::spawn(async move {
        let signals = transfer.signals();
        while signals.sleep(interval).await {
            if let Some(outcome) = transfer.poll().await {
                signals.completion.complete(outcome);
                return;
            }
        }
    });
    tokio::spawn(async move {
        if let Err(error) = task.await {
            tracing::error!("Polling a remote download stopped unexpectedly: {error}");
            watched
                .signals()
                .completion
                .complete(Err(STOPPED_UNEXPECTEDLY_MESSAGE.to_owned()));
        }
    });
}

/// `root/[category/]relative` as the remote host sees it: an absolute part replaces what
/// came before it, and backslashes become slashes.
pub(crate) fn remote_path(root: &str, category: Option<&str>, relative: &str) -> String {
    let mut path = root.to_owned();
    let category = category.filter(|category| !category.trim().is_empty());
    for part in category.into_iter().chain([relative]) {
        if part.starts_with('/') || path.is_empty() {
            part.clone_into(&mut path);
        } else if !part.is_empty() {
            if !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(part);
        }
    }
    path.replace('\\', "/")
}

/// The directory part of a remote path: `""` for a bare name, `None` for an empty path or
/// the root itself.
pub(crate) fn remote_parent(path: &str) -> Option<&str> {
    match path.rfind('/') {
        _ if path.is_empty() || path == "/" => None,
        None => Some(""),
        Some(0) => Some("/"),
        Some(index) => Some(&path[..index]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_path_joins_root_category_and_relative_path() {
        assert_eq!(
            remote_path("/downloads", Some("movies"), "Movie/file.mkv"),
            "/downloads/movies/Movie/file.mkv"
        );
        assert_eq!(
            remote_path("/downloads/", None, "Movie/file.mkv"),
            "/downloads/Movie/file.mkv"
        );
        assert_eq!(
            remote_path("/downloads", Some("  "), "Movie\\file.mkv"),
            "/downloads/Movie/file.mkv"
        );
    }

    #[test]
    fn absolute_parts_replace_the_root() {
        assert_eq!(
            remote_path("/downloads", Some("/other"), "Movie/file.mkv"),
            "/other/Movie/file.mkv"
        );
        assert_eq!(remote_path("", None, "Movie/file.mkv"), "Movie/file.mkv");
    }

    #[test]
    fn parent_of_remote_paths() {
        assert_eq!(remote_parent("/a/b/file.mkv"), Some("/a/b"));
        assert_eq!(remote_parent("/file.mkv"), Some("/"));
        assert_eq!(remote_parent("file.mkv"), Some(""));
        assert_eq!(remote_parent(""), None);
        assert_eq!(remote_parent("/"), None);
    }

    #[tokio::test]
    async fn begin_succeeds_once() {
        let signals = Signals::new();
        assert!(signals.begin().is_ok());
        let again = signals.begin().expect_err("second start");
        assert_eq!(again.message(), "The download was already started");
    }

    #[tokio::test]
    async fn begin_after_cancel_fails_with_the_cancelled_message() {
        let signals = Signals::new();
        signals.cancel();
        let error = signals.begin().expect_err("cancelled");
        assert_eq!(error.message(), CANCELLED_MESSAGE);
        assert_eq!(
            signals.completion.subscribe().outcome(),
            Some(Err(CANCELLED_MESSAGE.to_owned()))
        );
    }

    #[tokio::test]
    async fn waiting_for_a_file_finds_it_or_gives_up() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("file.mkv");
        let signals = Signals::new();
        assert_eq!(
            signals.wait_for_file(&path, Duration::ZERO).await,
            Some(false)
        );
        std::fs::write(&path, b"x").expect("file");
        assert_eq!(
            signals.wait_for_file(&path, Duration::ZERO).await,
            Some(true)
        );
        std::fs::remove_file(&path).expect("remove");
        signals.cancel.cancel();
        assert_eq!(
            signals.wait_for_file(&path, Duration::from_secs(5)).await,
            None
        );
    }
}

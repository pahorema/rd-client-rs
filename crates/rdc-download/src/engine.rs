//! The contract every download engine implements.

use std::fmt;

use async_trait::async_trait;
use tokio::sync::watch;

/// Completion message of a transfer stopped through [`DownloadEngine::cancel`].
pub const CANCELLED_MESSAGE: &str = "The download was cancelled";

/// Completion message of a transfer left to its download client through
/// [`DownloadEngine::detach`].
pub const DETACHED_MESSAGE: &str = "The download was left to the download client";

/// Completion message used when an engine went away without reporting an outcome.
pub const STOPPED_UNEXPECTEDLY_MESSAGE: &str = "The download stopped unexpectedly";

/// Final outcome of a transfer: `Err` carries the user-visible error message.
pub type Outcome = Result<(), String>;

/// A snapshot of a transfer's progress.
///
/// `bytes_total` is `0` while the size is unknown. Engines without byte counts (the symlink
/// engine) report attempts instead, as documented on the engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Progress {
    /// Bytes transferred so far.
    pub bytes_done: u64,
    /// Expected total size in bytes, `0` when unknown.
    pub bytes_total: u64,
    /// Current transfer speed in bytes per second.
    pub speed: u64,
}

/// Error returned by [`DownloadEngine::start`]; its message is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct StartError {
    message: String,
}

impl StartError {
    /// Creates an error carrying a user-visible message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The user-visible message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Converts the error into its message.
    pub fn into_message(self) -> String {
        self.message
    }
}

/// A download engine moving one remote file to one local path.
///
/// # Contract
///
/// The life cycle of a transfer:
///
/// 1. The caller constructs the engine with everything it needs (link, local path, plain
///    configuration values) and keeps it, usually as `Box<dyn DownloadEngine>` or
///    `Arc<dyn DownloadEngine>` in a set of active transfers.
/// 2. [`DownloadEngine::start`] is called **once**. It resolves as soon as the engine has an
///    identity for the transfer — the *remote id* the caller persists so a later run can find
///    the transfer again. It never waits for the transfer itself: the internal HTTP engine
///    answers immediately with a random id, remote engines answer once their backend accepted
///    (or adopted) the task, and the symlink engine answers once the source file was found and
///    linked (which *is* its whole transfer).
/// 3. While running, the engine publishes [`Progress`] through a [`watch`] channel
///    ([`DownloadEngine::progress`]). Remote engines poll their backend themselves (about once
///    per second); the caller never drives polling.
/// 4. The engine finishes **exactly once** through its [`Completion`]:
///    `Ok(())` on success, `Err(message)` with a user-visible message otherwise. A failing
///    `start` also resolves the completion with the same message, so a caller that only watches
///    completions still sees every transfer end.
/// 5. [`DownloadEngine::cancel`], [`DownloadEngine::detach`], [`DownloadEngine::pause`] and
///    [`DownloadEngine::resume`] may be called at any time and in any order, any number of
///    times. They never fail the caller: backend errors are logged. `cancel` returns once the
///    engine stopped transferring and cleaned up what it owns, and a cancelled transfer
///    completes with [`CANCELLED_MESSAGE`] unless it had already completed. `detach` stops
///    following the transfer but leaves a remote downloader's job running, so a later run
///    adopts it through the persisted remote id; such a transfer completes with
///    [`DETACHED_MESSAGE`] unless it had already completed. An engine without a remote job
///    cancels instead.
///
/// # Implementing an engine
///
/// Keep a [`watch::Sender<Progress>`] and a [`CompletionSender`] (from
/// [`completion_channel`]) in the engine's shared state and hand out subscriptions from
/// [`progress`](DownloadEngine::progress) and [`completion`](DownloadEngine::completion).
/// [`CompletionSender::complete`] keeps the first outcome, so racing paths (a finishing
/// transfer and a cancel) may both report; a sender dropped without reporting resolves the
/// completion with [`STOPPED_UNEXPECTEDLY_MESSAGE`]. Methods take `&self` so an engine can be
/// shared while `start` is still running; use interior mutability for engine state.
#[async_trait]
pub trait DownloadEngine: Send + Sync + fmt::Debug {
    /// Starts (or adopts) the transfer and returns the remote id to persist.
    ///
    /// Must be called at most once; later calls fail without side effects. A failure also
    /// resolves the engine's [`Completion`] with the error message.
    async fn start(&self) -> Result<String, StartError>;

    /// Subscribes to progress updates.
    fn progress(&self) -> watch::Receiver<Progress>;

    /// Returns a handle on the transfer's single outcome.
    fn completion(&self) -> Completion;

    /// Stops the transfer and releases what the engine owns; idempotent, never fails.
    async fn cancel(&self);

    /// Stops following the transfer without touching the remote downloader's job, so a later
    /// run can adopt it through the remote id; idempotent, never fails.
    ///
    /// The default cancels: an engine whose transfer lives only in this process (the internal
    /// HTTP and symlink engines) leaves nothing behind that a later run could adopt.
    async fn detach(&self) {
        self.cancel().await;
    }

    /// Suspends the transfer; idempotent, never fails. Engines that cannot pause ignore it.
    async fn pause(&self);

    /// Continues a paused transfer; idempotent, never fails.
    async fn resume(&self);
}

/// Creates a linked completion sender and receiver.
pub fn completion_channel() -> (CompletionSender, Completion) {
    let (tx, rx) = watch::channel(None);
    (CompletionSender { tx }, Completion { rx })
}

/// Reports a transfer's outcome; the first reported outcome wins.
///
/// Dropping the sender without reporting resolves the completion with
/// [`STOPPED_UNEXPECTEDLY_MESSAGE`], so waiters never hang.
#[derive(Debug)]
pub struct CompletionSender {
    tx: watch::Sender<Option<Outcome>>,
}

impl CompletionSender {
    /// Reports the outcome; returns `false` when an outcome was already reported.
    pub fn complete(&self, outcome: Outcome) -> bool {
        let mut outcome = Some(outcome);
        self.tx.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = outcome.take();
            true
        })
    }

    /// Whether an outcome was reported.
    pub fn is_completed(&self) -> bool {
        self.tx.borrow().is_some()
    }

    /// Returns another receiver of the same outcome.
    pub fn subscribe(&self) -> Completion {
        Completion {
            rx: self.tx.subscribe(),
        }
    }
}

impl Drop for CompletionSender {
    fn drop(&mut self) {
        self.complete(Err(STOPPED_UNEXPECTEDLY_MESSAGE.to_owned()));
    }
}

/// Receives a transfer's outcome; cheap to clone.
#[derive(Debug, Clone)]
pub struct Completion {
    rx: watch::Receiver<Option<Outcome>>,
}

impl Completion {
    /// The outcome if the transfer finished, without waiting.
    pub fn outcome(&self) -> Option<Outcome> {
        self.rx.borrow().clone()
    }

    /// Whether the transfer finished.
    pub fn is_finished(&self) -> bool {
        self.rx.borrow().is_some()
    }

    /// Waits for the outcome.
    pub async fn wait(&mut self) -> Outcome {
        // The sender reports an outcome before its channel closes, so once this returns the
        // value is set either way.
        let _ = self.rx.wait_for(Option::is_some).await;
        self.rx
            .borrow()
            .clone()
            .unwrap_or_else(|| Err(STOPPED_UNEXPECTEDLY_MESSAGE.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completion_resolves_with_first_outcome_only() {
        let (sender, mut completion) = completion_channel();
        assert!(!completion.is_finished());
        assert!(sender.complete(Err("boom".to_owned())));
        assert!(!sender.complete(Ok(())));
        assert_eq!(completion.wait().await, Err("boom".to_owned()));
        assert_eq!(completion.outcome(), Some(Err("boom".to_owned())));
    }

    #[tokio::test]
    async fn completion_is_visible_to_every_subscriber() {
        let (sender, first) = completion_channel();
        let mut second = sender.subscribe();
        let mut third = first.clone();
        sender.complete(Ok(()));
        assert_eq!(second.wait().await, Ok(()));
        assert_eq!(third.wait().await, Ok(()));
    }

    #[tokio::test]
    async fn dropped_sender_reports_unexpected_stop() {
        let (sender, mut completion) = completion_channel();
        drop(sender);
        assert_eq!(
            completion.wait().await,
            Err(STOPPED_UNEXPECTEDLY_MESSAGE.to_owned())
        );
    }

    #[tokio::test]
    async fn dropped_sender_keeps_reported_outcome() {
        let (sender, mut completion) = completion_channel();
        sender.complete(Ok(()));
        drop(sender);
        assert_eq!(completion.wait().await, Ok(()));
    }

    #[tokio::test]
    async fn waiting_resolves_when_outcome_arrives_later() {
        let (sender, mut completion) = completion_channel();
        let waiter = tokio::spawn(async move { completion.wait().await });
        tokio::task::yield_now().await;
        sender.complete(Err("late".to_owned()));
        assert_eq!(waiter.await.expect("waiter"), Err("late".to_owned()));
    }

    #[test]
    fn start_error_displays_its_message() {
        let error = StartError::new("Invalid download link");
        assert_eq!(error.to_string(), "Invalid download link");
        assert_eq!(error.message(), "Invalid download link");
        assert_eq!(error.into_message(), "Invalid download link");
    }

    const _: () = {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Completion>();
        assert_send_sync::<CompletionSender>();
    };
}

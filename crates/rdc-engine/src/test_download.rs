//! Controlled transfer used by registry and diagnostic tests.

use async_trait::async_trait;
use rdc_download::{
    Completion, CompletionSender, DownloadEngine, Progress, StartError, completion_channel,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Notify, watch};

#[derive(Debug)]
pub(crate) struct TestDownload {
    pub progress: watch::Sender<Progress>,
    pub completion: CompletionSender,
    pub paused: AtomicUsize,
    pub resumed: AtomicUsize,
    pub cancelled: AtomicUsize,
    pub cancel_started: Notify,
    pub cancel_release: Notify,
    pub delayed_cancel: bool,
}

impl TestDownload {
    pub(crate) fn new(delayed_cancel: bool) -> Self {
        Self {
            progress: watch::Sender::new(Progress::default()),
            completion: completion_channel().0,
            paused: AtomicUsize::new(0),
            resumed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            cancel_started: Notify::new(),
            cancel_release: Notify::new(),
            delayed_cancel,
        }
    }
}

#[async_trait]
impl DownloadEngine for TestDownload {
    async fn start(&self) -> Result<String, StartError> {
        Ok("test-transfer".to_owned())
    }
    fn progress(&self) -> watch::Receiver<Progress> {
        self.progress.subscribe()
    }
    fn completion(&self) -> Completion {
        self.completion.subscribe()
    }
    async fn cancel(&self) {
        self.cancelled.fetch_add(1, Ordering::Relaxed);
        self.cancel_started.notify_one();
        if self.delayed_cancel {
            self.cancel_release.notified().await;
        }
        self.completion
            .complete(Err("The download was cancelled".to_owned()));
    }
    async fn pause(&self) {
        self.paused.fetch_add(1, Ordering::Relaxed);
    }
    async fn resume(&self) {
        self.resumed.fetch_add(1, Ordering::Relaxed);
    }
}

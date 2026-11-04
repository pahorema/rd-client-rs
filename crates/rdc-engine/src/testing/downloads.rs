//! Controllable network-free host transfers and a queued engine factory.

use crate::{DownloadFactory, DownloadRequest, EngineError};
use async_trait::async_trait;
use rdc_download::{
    Completion, CompletionSender, DownloadEngine, Outcome, Progress, StartError, completion_channel,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// A cloneable fake whose progress, completion and method counters share one transfer.
#[derive(Debug, Clone)]
pub struct FakeDownloadEngine {
    state: Arc<State>,
}
#[derive(Debug)]
struct State {
    result: Mutex<Result<String, String>>,
    progress: watch::Sender<Progress>,
    completion: CompletionSender,
    gate: Mutex<CancellationToken>,
    cancel: CancellationToken,
    started: AtomicUsize,
    paused: AtomicUsize,
    resumed: AtomicUsize,
    cancelled: AtomicUsize,
    is_cancelled: AtomicBool,
}
impl Default for FakeDownloadEngine {
    fn default() -> Self {
        Self::new()
    }
}
impl FakeDownloadEngine {
    /// Creates an immediately startable transfer that completes only when explicitly controlled.
    pub fn new() -> Self {
        let gate = CancellationToken::new();
        gate.cancel();
        Self {
            state: Arc::new(State {
                result: Mutex::new(Ok(uuid::Uuid::new_v4().to_string())),
                progress: watch::Sender::new(Progress::default()),
                completion: completion_channel().0,
                gate: Mutex::new(gate),
                cancel: CancellationToken::new(),
                started: AtomicUsize::new(0),
                paused: AtomicUsize::new(0),
                resumed: AtomicUsize::new(0),
                cancelled: AtomicUsize::new(0),
                is_cancelled: AtomicBool::new(false),
            }),
        }
    }
    /// Sets the identifier or failure returned by the first start.
    pub fn set_start_result(&self, result: Result<String, String>) {
        *self.state.result.lock().expect("fake start lock poisoned") = result;
    }
    /// Publishes byte progress and instantaneous speed.
    pub fn set_progress(&self, progress: Progress) {
        self.state.progress.send_replace(progress);
    }
    /// Reports the first completion outcome; subsequent outcomes are ignored.
    pub fn complete(&self, outcome: Outcome) {
        self.state.completion.complete(outcome);
    }
    /// Holds the next start until released or cancelled.
    pub fn block_start(&self) {
        *self
            .state
            .gate
            .lock()
            .expect("fake start gate lock poisoned") = CancellationToken::new();
    }
    /// Releases a held start; releasing an already open gate is harmless.
    pub fn release_start(&self) {
        self.state
            .gate
            .lock()
            .expect("fake start gate lock poisoned")
            .cancel();
    }
    /// Number of accepted start attempts.
    pub fn started(&self) -> usize {
        self.state.started.load(Ordering::Relaxed)
    }
    /// Number of pause invocations.
    pub fn paused(&self) -> usize {
        self.state.paused.load(Ordering::Relaxed)
    }
    /// Number of resume invocations.
    pub fn resumed(&self) -> usize {
        self.state.resumed.load(Ordering::Relaxed)
    }
    /// One after cancellation, zero beforehand.
    pub fn cancelled(&self) -> usize {
        self.state.cancelled.load(Ordering::Relaxed)
    }
}
#[async_trait]
impl DownloadEngine for FakeDownloadEngine {
    async fn start(&self) -> Result<String, StartError> {
        if self.state.started.fetch_add(1, Ordering::AcqRel) != 0 {
            return Err(StartError::new("The download was already started"));
        }
        let gate = self
            .state
            .gate
            .lock()
            .expect("fake start gate lock poisoned")
            .clone();
        tokio::select! { biased; () = self.state.cancel.cancelled() => return Err(StartError::new(rdc_download::CANCELLED_MESSAGE)), () = gate.cancelled() => {} }
        let result = self
            .state
            .result
            .lock()
            .expect("fake start lock poisoned")
            .clone();
        result.map_err(|message| {
            self.state.completion.complete(Err(message.clone()));
            StartError::new(message)
        })
    }
    fn progress(&self) -> watch::Receiver<Progress> {
        self.state.progress.subscribe()
    }
    fn completion(&self) -> Completion {
        self.state.completion.subscribe()
    }
    async fn cancel(&self) {
        if !self.state.is_cancelled.swap(true, Ordering::AcqRel) {
            self.state.cancelled.fetch_add(1, Ordering::Relaxed);
        }
        self.state.cancel.cancel();
        self.state
            .completion
            .complete(Err(rdc_download::CANCELLED_MESSAGE.to_owned()));
    }
    async fn pause(&self) {
        self.state.paused.fetch_add(1, Ordering::Relaxed);
    }
    async fn resume(&self) {
        self.state.resumed.fetch_add(1, Ordering::Relaxed);
    }
}

/// Returns queued fake transfers while capturing the exact requests supplied by the engine.
#[derive(Debug, Clone, Default)]
pub struct FakeDownloadFactory {
    state: Arc<Mutex<FactoryState>>,
}
#[derive(Debug, Default)]
struct FactoryState {
    queued: VecDeque<FakeDownloadEngine>,
    failures: VecDeque<String>,
    engines: Vec<FakeDownloadEngine>,
    requests: Vec<DownloadRequest>,
}
impl FakeDownloadFactory {
    /// Creates an empty factory; unconfigured calls create a pending fake transfer.
    pub fn new() -> Self {
        Self::default()
    }
    /// Supplies the engine returned by the next successful construction.
    pub fn push(&self, engine: FakeDownloadEngine) {
        self.state
            .lock()
            .expect("fake factory lock poisoned")
            .queued
            .push_back(engine);
    }
    /// Makes the next construction fail without consuming a queued transfer.
    pub fn queue_failure(&self, message: String) {
        self.state
            .lock()
            .expect("fake factory lock poisoned")
            .failures
            .push_back(message);
    }
    /// All engines returned by successful constructions, in order.
    pub fn engines(&self) -> Vec<FakeDownloadEngine> {
        self.state
            .lock()
            .expect("fake factory lock poisoned")
            .engines
            .clone()
    }
    /// All attempted construction requests, including failures.
    pub fn requests(&self) -> Vec<DownloadRequest> {
        self.state
            .lock()
            .expect("fake factory lock poisoned")
            .requests
            .clone()
    }
}
#[async_trait]
impl DownloadFactory for FakeDownloadFactory {
    async fn create(
        &self,
        request: DownloadRequest,
    ) -> Result<Arc<dyn DownloadEngine>, EngineError> {
        let mut state = self.state.lock().expect("fake factory lock poisoned");
        state.requests.push(request);
        if let Some(message) = state.failures.pop_front() {
            return Err(EngineError::Message(message));
        }
        let engine = state.queued.pop_front().unwrap_or_default();
        state.engines.push(engine.clone());
        Ok(Arc::new(engine))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_releases_a_blocked_start_and_reports_completion_once() {
        let engine = FakeDownloadEngine::new();
        engine.block_start();
        let start = engine.clone();
        let task = tokio::spawn(async move { start.start().await });
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        engine.cancel().await;
        engine.cancel().await;
        assert!(task.await.expect("start task").is_err());
        assert_eq!(engine.cancelled(), 1);
        let mut completion = engine.completion();
        assert_eq!(
            completion.wait().await,
            Err(rdc_download::CANCELLED_MESSAGE.to_owned())
        );
        engine.complete(Ok(()));
        assert!(completion.outcome().expect("outcome").is_err());
    }
    #[tokio::test]
    async fn progress_and_controls_share_state_across_clones() {
        let engine = FakeDownloadEngine::new();
        let other = engine.clone();
        engine.set_progress(Progress {
            bytes_done: 4,
            bytes_total: 10,
            speed: 2,
        });
        assert_eq!(other.progress().borrow().bytes_done, 4);
        other.start().await.expect("start");
        other.pause().await;
        other.resume().await;
        assert_eq!(engine.started(), 1);
        assert_eq!(engine.paused(), 1);
        assert_eq!(engine.resumed(), 1);
        engine.complete(Ok(()));
        assert_eq!(other.completion().wait().await, Ok(()));
    }
}

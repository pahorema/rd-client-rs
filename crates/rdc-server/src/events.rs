//! Authenticated server-sent updates for the web client.

use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    extract::State,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use rdc_engine::{Engine, EngineEvent};
use tokio::{
    sync::{mpsc, watch},
    task::AbortHandle,
    time::MissedTickBehavior,
};
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    app::AppState,
    dto::{TorrentDto, Variant},
};

/// Literal routes used for case-insensitive matching.
pub(crate) const PATHS: &[&str] = &["/api/events"];

/// Register the event stream.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(PATHS[0], get(stream))
}

/// Serialized torrent list, as sent in an `update` event.
type Snapshot = Option<Arc<str>>;

/// Torrent list updates shared by every open event stream: one task reads the torrents
/// once a second while at least one stream is open, and every stream sends its result.
#[derive(Clone, Debug)]
pub(crate) struct Updates(Arc<Mutex<Hub>>);

#[derive(Debug)]
struct Hub {
    streams: usize,
    snapshots: Arc<watch::Sender<Snapshot>>,
    producer: Option<AbortHandle>,
}

impl Default for Updates {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Hub {
            streams: 0,
            snapshots: Arc::new(watch::Sender::new(None)),
            producer: None,
        })))
    }
}

impl Updates {
    /// Registers a stream, starting the shared reader for the first one.
    fn connect(&self, engine: &Engine) -> (Connected, watch::Receiver<Snapshot>) {
        let mut hub = self.0.lock().expect("event connection lock poisoned");
        hub.streams += 1;
        if hub.producer.is_none() {
            hub.snapshots.send_replace(None);
            engine.set_ui_connected(true);
            let producer = tokio::spawn(produce(engine.clone(), hub.snapshots.clone()));
            hub.producer = Some(producer.abort_handle());
        }
        let mut snapshots = hub.snapshots.subscribe();
        // Send the latest list right away when the reader already runs.
        snapshots.mark_changed();
        (
            Connected {
                updates: self.clone(),
                engine: engine.clone(),
            },
            snapshots,
        )
    }
}

/// Unregisters a stream when dropped, stopping the shared reader after the last one.
struct Connected {
    updates: Updates,
    engine: Engine,
}

impl Drop for Connected {
    fn drop(&mut self) {
        let mut hub = self
            .updates
            .0
            .lock()
            .expect("event connection lock poisoned");
        hub.streams -= 1;
        if hub.streams == 0 {
            if let Some(producer) = hub.producer.take() {
                producer.abort();
            }
            self.engine.set_ui_connected(false);
        }
    }
}

async fn produce(engine: Engine, snapshots: Arc<watch::Sender<Snapshot>>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match snapshot(&engine).await {
            Ok(data) => {
                snapshots.send_replace(Some(data));
            }
            Err(error) => tracing::warn!(%error, "Cannot prepare torrent update"),
        }
    }
}

async fn snapshot(engine: &Engine) -> anyhow::Result<Arc<str>> {
    let data: Vec<_> = engine
        .torrents()
        .await?
        .into_iter()
        .map(|torrent| TorrentDto::new(engine, torrent, Variant::Update))
        .collect();
    Ok(serde_json::to_string(&data)?.into())
}

async fn stream(State(state): State<AppState>) -> impl IntoResponse {
    let mut events = state.engine.subscribe();
    let (connected, mut snapshots) = state.updates.connect(&state.engine);
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(16);
    let shutdown = state.shutdown;
    tokio::spawn(async move {
        let _connected = connected;
        loop {
            let event = tokio::select! {
                () = tx.closed() => break,
                () = shutdown.cancelled() => break,
                changed = snapshots.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let Some(data) = snapshots.borrow_and_update().clone() else { continue };
                    Event::default().event("update").data(&*data)
                }
                result = events.recv() => {
                    let event = match result {
                        Ok(EngineEvent::DiskSpaceStatus(status)) => Event::default().event("diskSpaceStatus").json_data(status),
                        Ok(EngineEvent::RateLimitStatus(status)) => Event::default().event("rateLimitStatus").json_data(status),
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    };
                    match event {
                        Ok(event) => event,
                        Err(error) => {
                            tracing::warn!(%error, "Cannot serialize event");
                            continue;
                        }
                    }
                }
            };
            // A client that stopped reading fills the channel; shutdown must still end it.
            tokio::select! {
                sent = tx.send(Ok(event)) => if sent.is_err() { break },
                () = shutdown.cancelled() => break,
            }
        }
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdc_engine::testing::TestEngine;

    #[tokio::test]
    async fn streams_share_one_reader_that_stops_with_the_last_stream() {
        let fixture = TestEngine::new().await.unwrap();
        let updates = Updates::default();
        let (first, mut first_rx) = updates.connect(&fixture.engine);
        let (second, mut second_rx) = updates.connect(&fixture.engine);
        let producer = updates.0.lock().unwrap().producer.clone().unwrap();
        for receiver in [&mut first_rx, &mut second_rx] {
            let snapshot = receiver.wait_for(Option::is_some).await.unwrap().clone();
            assert_eq!(snapshot.as_deref(), Some("[]"));
        }
        drop(first);
        assert!(!producer.is_finished());
        drop(second);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !producer.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(updates.0.lock().unwrap().producer.is_none());
    }
}

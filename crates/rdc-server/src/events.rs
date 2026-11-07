//! Authenticated server-sent updates for the web client.

use std::{convert::Infallible, time::Duration};

use axum::{
    Router,
    extract::State,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use rdc_engine::EngineEvent;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    app::{self, AppState},
    dto::{TorrentDto, Variant},
    http::ApiError,
};

/// Literal routes used for case-insensitive matching.
pub(crate) const PATHS: &[&str] = &["/api/events"];

/// Register the event stream.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(PATHS[0], get(stream))
}

struct Connected(AppState);

impl Drop for Connected {
    fn drop(&mut self) {
        let mut count = self
            .0
            .connections
            .lock()
            .expect("event connection lock poisoned");
        *count -= 1;
        if *count == 0 {
            self.0.engine.set_ui_connected(false);
        }
    }
}

async fn snapshot(state: &AppState) -> Result<Event, ApiError> {
    let torrents = state.engine.torrents().await.map_err(app::internal)?;
    let data: Vec<_> = torrents
        .into_iter()
        .map(|torrent| TorrentDto::new(&state.engine, torrent, Variant::Update))
        .collect();
    Event::default()
        .event("update")
        .json_data(data)
        .map_err(app::internal)
}

async fn stream(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let mut events = state.engine.subscribe();
    let first = snapshot(&state).await?;
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(16);
    {
        let mut count = state
            .connections
            .lock()
            .expect("event connection lock poisoned");
        *count += 1;
        state.engine.set_ui_connected(true);
    }
    tokio::spawn(async move {
        let _connected = Connected(state.clone());
        if tx.send(Ok(first)).await.is_err() {
            return;
        }
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        ticker.tick().await;
        loop {
            tokio::select! {
                () = tx.closed() => break,
                () = state.shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    match snapshot(&state).await {
                        Ok(event) => if tx.send(Ok(event)).await.is_err() { break; },
                        Err(error) => tracing::warn!(%error, "Cannot prepare torrent update"),
                    }
                }
                result = events.recv() => {
                    let event = match result {
                        Ok(EngineEvent::DiskSpaceStatus(status)) => Event::default().event("diskSpaceStatus").json_data(status),
                        Ok(EngineEvent::RateLimitStatus(status)) => Event::default().event("rateLimitStatus").json_data(status),
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    };
                    match event {
                        Ok(event) => if tx.send(Ok(event)).await.is_err() { break; },
                        Err(error) => tracing::warn!(%error, "Cannot serialize event"),
                    }
                }
            }
        }
    });
    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()))
}

//! HTTP router assembly and shared application dependencies.

use std::{
    fmt::Display,
    sync::{Arc, Mutex},
};

use axum::{Json, Router, extract::rejection::JsonRejection, http::StatusCode, middleware};
use rdc_engine::Engine;
use rdc_store::Store;
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, Error as _},
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    accounts,
    auth::{AuthPolicy, Sessions, require_auth},
    events,
    http::{ApiError, AppBuilder},
    logging::LogControl,
    preferences, qbittorrent, torrents,
};

/// Services the web API handlers work with.
#[derive(Clone, Debug)]
pub struct Backend {
    /// Torrent lifecycle and settings facade.
    pub engine: Engine,
    /// Account persistence.
    pub store: Store,
    /// Session signer used by the account endpoints.
    pub sessions: Sessions,
    /// Live authorization policy.
    pub policy: AuthPolicy,
}

/// Shared state of the web API handlers.
#[derive(Clone, Debug)]
pub(crate) struct AppState {
    pub(crate) engine: Engine,
    pub(crate) store: Store,
    pub(crate) sessions: Sessions,
    pub(crate) policy: AuthPolicy,
    /// Number of currently connected event streams.
    pub(crate) connections: Arc<Mutex<usize>>,
    /// Ends open event streams during graceful shutdown.
    pub(crate) shutdown: CancellationToken,
}

/// Map any failure to a 500 response carrying its message.
pub(crate) fn internal(error: impl Display) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

/// Unwrap a JSON body, answering 400 with an empty body when it does not parse.
pub(crate) fn body<T: DeserializeOwned>(
    input: Result<Json<T>, JsonRejection>,
) -> Result<T, ApiError> {
    input
        .map(|Json(value)| value)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, ""))
}

/// Unwrap a JSON body that must not be `null`.
pub(crate) fn required<T: DeserializeOwned>(
    input: Result<Json<Option<T>>, JsonRejection>,
) -> Result<T, ApiError> {
    body(input)?.ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, ""))
}

/// A JSON object whose property names match the target fields regardless of case.
#[derive(Debug)]
pub(crate) struct CaseInsensitive<T>(pub(crate) T);

impl<'de, T: DeserializeOwned> Deserialize<'de> for CaseInsensitive<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let Value::Object(fields) = Value::deserialize(deserializer)? else {
            return Err(D::Error::custom("expected a JSON object"));
        };
        let normalized = fields
            .into_iter()
            .map(|(key, value)| (key.to_ascii_lowercase(), value))
            .collect();
        serde_json::from_value(Value::Object(normalized))
            .map(Self)
            .map_err(D::Error::custom)
    }
}

/// Build the complete application: the web API, the qBittorrent-compatible API and the UI.
///
/// Cancelling `shutdown` ends every open event stream.
pub fn build(
    backend: Backend,
    base_path: &str,
    logs: LogControl,
    shutdown: CancellationToken,
) -> Router {
    let Backend {
        engine,
        store,
        sessions,
        policy,
    } = backend;
    let torrent_client = qbittorrent::router(engine.clone(), sessions.clone(), policy.clone());
    let protected = Router::new()
        .merge(accounts::protected_routes())
        .merge(preferences::routes())
        .merge(torrents::routes())
        .merge(events::routes())
        .route_layer(middleware::from_fn_with_state(policy.clone(), require_auth));
    let state = AppState {
        engine,
        store,
        sessions,
        policy,
        connections: Arc::new(Mutex::new(0)),
        shutdown,
    };
    AppBuilder::new(base_path, logs)
        .merge(accounts::public_routes(), accounts::PUBLIC_PATHS)
        .merge(
            protected,
            &[
                accounts::PROTECTED_PATHS,
                preferences::PATHS,
                torrents::PATHS,
                events::PATHS,
            ]
            .concat(),
        )
        .merge(torrent_client, qbittorrent::PATHS)
        .build(state)
}

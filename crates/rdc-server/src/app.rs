//! HTTP router assembly and shared application dependencies.

use std::{
    any::Any,
    fmt::{self, Display},
};

use axum::{Json, Router, extract::rejection::JsonRejection, http::StatusCode, middleware};
use rdc_engine::{Engine, EngineError};
use rdc_store::Store;
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, DeserializeSeed, IntoDeserializer, MapAccess, Visitor},
};
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
    /// Torrent list updates shared by the open event streams.
    pub(crate) updates: events::Updates,
    /// Ends open event streams during graceful shutdown.
    pub(crate) shutdown: CancellationToken,
}

/// Map a failure to a 500 response.
///
/// Engine validation and provider messages are written for the user and are returned;
/// any other cause is logged and answered with a generic text, since it can name files,
/// URLs or database details.
pub(crate) fn internal(error: impl Display + 'static) -> ApiError {
    match (&error as &dyn Any).downcast_ref::<EngineError>() {
        // Transport errors carry the request URL, which can hold an API key.
        Some(EngineError::Provider(rdc_debrid::DebridError::Transport(_))) | None => {
            ApiError::internal(error)
        }
        Some(EngineError::Message(_) | EngineError::Provider(_)) => {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
        Some(_) => ApiError::internal(error),
    }
}

/// Unwrap a JSON body, answering 400 with an empty body when it does not parse, or 413
/// when it exceeds the route's body limit.
pub(crate) fn body<T: DeserializeOwned>(
    input: Result<Json<T>, JsonRejection>,
) -> Result<T, ApiError> {
    input.map(|Json(value)| value).map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(rejection.status(), rejection.body_text())
        } else {
            ApiError::new(StatusCode::BAD_REQUEST, "")
        }
    })
}

/// Unwrap a JSON body that must not be `null`.
pub(crate) fn required<T: DeserializeOwned>(
    input: Result<Json<Option<T>>, JsonRejection>,
) -> Result<T, ApiError> {
    body(input)?.ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, ""))
}

/// A JSON object whose property names match the target fields regardless of case.
///
/// Top-level keys are lowercased while the object is read, so field names of `T` must be
/// lowercase; values and unknown fields stream through without building a JSON tree.
#[derive(Debug)]
pub(crate) struct CaseInsensitive<T>(pub(crate) T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for CaseInsensitive<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(LowercaseKeys(deserializer)).map(Self)
    }
}

/// Deserializer presenting an object with lowercase keys.
struct LowercaseKeys<D>(D);

impl<'de, D: Deserializer<'de>> Deserializer<'de> for LowercaseKeys<D> {
    type Error = D::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        self.0.deserialize_map(ObjectVisitor(visitor))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.0.deserialize_map(ObjectVisitor(visitor))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map enum identifier
        ignored_any
    }
}

/// Accepts only objects and hands them on with lowercase keys.
struct ObjectVisitor<V>(V);

impl<'de, V: Visitor<'de>> Visitor<'de> for ObjectVisitor<V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        self.0.visit_map(LowercaseMap(map))
    }
}

struct LowercaseMap<A>(A);

impl<'de, A: MapAccess<'de>> MapAccess<'de> for LowercaseMap<A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        let Some(key) = self.0.next_key::<String>()? else {
            return Ok(None);
        };
        seed.deserialize(key.to_ascii_lowercase().into_deserializer())
            .map(Some)
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, A::Error> {
        self.0.next_value_seed(seed)
    }

    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
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
        updates: events::Updates::default(),
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

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, response::IntoResponse};

    use super::*;

    async fn body(error: ApiError) -> (StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn internal_errors_hide_details_but_keep_engine_messages() {
        let failure = std::io::Error::other("/srv/db/rd-client.db is locked");
        assert_eq!(
            body(internal(failure)).await,
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_owned()
            )
        );
        let store = EngineError::Io(std::io::Error::other("/srv/downloads"));
        assert_eq!(body(internal(store)).await.1, "Internal server error");
        let message = EngineError::Message("Invalid magnet link".to_owned());
        assert_eq!(body(internal(message)).await.1, "Invalid magnet link");
        let provider =
            EngineError::Provider(rdc_debrid::DebridError::Provider("Bad token".to_owned()));
        assert_eq!(body(internal(provider)).await.1, "Bad token");
    }
}

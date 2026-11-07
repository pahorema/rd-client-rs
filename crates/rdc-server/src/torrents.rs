//! Torrent management, availability checks and transfer diagnostics.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use rdc_engine::{RateLimitStatus, disk_space::DiskSpaceStatus};
use uuid::Uuid;

use crate::{
    app::{self, AppState},
    dto::{TorrentDto, Variant},
    http::ApiError,
};

/// Literal routes used by base-path and case-insensitive request handling.
pub(crate) const PATHS: &[&str] = &[
    "/api/torrents",
    "/api/torrents/get/{id}",
    "/api/torrents/diskspacestatus",
    "/api/torrents/ratelimitstatus",
    "/api/torrents/tick",
];

/// Register torrent API handlers.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(PATHS[0], get(list))
        .route(PATHS[1], get(detail))
        .route(PATHS[2], get(disk))
        .route(PATHS[3], get(rate))
        .route(PATHS[4], get(tick))
}

async fn list(State(s): State<AppState>) -> Result<Json<Vec<TorrentDto>>, ApiError> {
    let torrents = s.engine.torrents().await.map_err(app::internal)?;
    Ok(Json(
        torrents
            .into_iter()
            .map(|t| TorrentDto::new(&s.engine, t, Variant::List))
            .collect(),
    ))
}

async fn detail(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TorrentDto>, ApiError> {
    let id = valid_id(&id)?;
    let t = s
        .engine
        .torrent(id)
        .await
        .map_err(app::internal)?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, ""))?;
    Ok(Json(TorrentDto::new(&s.engine, t, Variant::Detail)))
}

async fn disk(State(s): State<AppState>) -> Result<Json<DiskSpaceStatus>, StatusCode> {
    s.engine
        .disk_space_status()
        .map(Json)
        .ok_or(StatusCode::NO_CONTENT)
}

async fn rate(State(s): State<AppState>) -> Json<RateLimitStatus> {
    Json(s.engine.rate_limit_status())
}

async fn tick(State(s): State<AppState>) -> Result<(), ApiError> {
    s.engine.tick_once().await.map_err(app::internal)
}

fn valid_id(value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value).map_err(|_| ApiError::new(StatusCode::NOT_FOUND, ""))
}

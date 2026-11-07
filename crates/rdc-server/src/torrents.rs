//! Torrent management, availability checks and transfer diagnostics.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use fancy_regex::Regex;
use rdc_debrid::AvailableFile;
use rdc_engine::{
    DeleteOptions, RateLimitStatus, TorrentTemplate, TorrentUpdate, disk_space::DiskSpaceStatus,
};
use rdc_model::{
    DownloadClient, TorrentDownloadAction, TorrentFinishedAction, TorrentHostDownloadAction,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    app::{self, AppState},
    dto::{TorrentDto, Variant},
    http::{ApiError, UPLOAD_BODY_LIMIT},
};

/// Literal routes used by base-path and case-insensitive request handling.
pub(crate) const PATHS: &[&str] = &[
    "/api/torrents",
    "/api/torrents/get/{id}",
    "/api/torrents/diskspacestatus",
    "/api/torrents/ratelimitstatus",
    "/api/torrents/tick",
    "/api/torrents/uploadmagnet",
    "/api/torrents/uploadfile",
    "/api/torrents/checkfilesmagnet",
    "/api/torrents/checkfiles",
    "/api/torrents/verifyregex",
    "/api/torrents/delete/{id}",
    "/api/torrents/retry/{id}",
    "/api/torrents/retrydownload/{id}",
    "/api/torrents/update",
];

/// Register torrent API handlers.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(PATHS[0], get(list))
        .route(PATHS[1], get(detail))
        .route(PATHS[2], get(disk))
        .route(PATHS[3], get(rate))
        .route(PATHS[4], get(tick))
        .route(PATHS[5], post(upload_magnet))
        .route(
            PATHS[6],
            post(upload_file).layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)),
        )
        .route(PATHS[7], post(check_magnet))
        .route(
            PATHS[8],
            post(check_file).layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)),
        )
        .route(PATHS[9], post(verify_regex))
        .route(PATHS[10], post(delete))
        .route(PATHS[11], post(retry))
        .route(PATHS[12], post(retry_download))
        .route(PATHS[13], put(update))
}

/// Per-torrent settings sent with a new torrent; missing fields take the template defaults.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct NewTorrent {
    category: Option<String>,
    host_download_action: TorrentHostDownloadAction,
    download_action: TorrentDownloadAction,
    finished_action: TorrentFinishedAction,
    finished_action_delay: i32,
    download_min_size: i32,
    include_regex: Option<String>,
    exclude_regex: Option<String>,
    download_manual_files: Option<String>,
    priority: Option<i32>,
    torrent_retry_attempts: i32,
    download_retry_attempts: i32,
    delete_on_error: i32,
    lifetime: i32,
    download_client: DownloadClient,
}

impl Default for NewTorrent {
    fn default() -> Self {
        let t = TorrentTemplate::default();
        Self {
            category: t.category,
            host_download_action: t.host_download_action,
            download_action: t.download_action,
            finished_action: t.finished_action,
            finished_action_delay: t.finished_action_delay,
            download_min_size: t.download_min_size,
            include_regex: t.include_regex,
            exclude_regex: t.exclude_regex,
            download_manual_files: t.download_manual_files,
            priority: t.priority,
            torrent_retry_attempts: t.torrent_retry_attempts,
            download_retry_attempts: t.download_retry_attempts,
            delete_on_error: t.delete_on_error,
            lifetime: t.lifetime,
            download_client: t.download_client,
        }
    }
}

impl From<NewTorrent> for TorrentTemplate {
    fn from(t: NewTorrent) -> Self {
        Self {
            category: t.category,
            host_download_action: t.host_download_action,
            download_action: t.download_action,
            finished_action: t.finished_action,
            finished_action_delay: t.finished_action_delay,
            download_min_size: t.download_min_size,
            include_regex: t.include_regex,
            exclude_regex: t.exclude_regex,
            download_manual_files: t.download_manual_files,
            priority: t.priority,
            torrent_retry_attempts: t.torrent_retry_attempts,
            download_retry_attempts: t.download_retry_attempts,
            delete_on_error: t.delete_on_error,
            lifetime: t.lifetime,
            download_client: t.download_client,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MagnetRequest {
    magnet_link: Option<String>,
    torrent: Option<NewTorrent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MagnetOnly {
    magnet_link: Option<String>,
}

/// JSON carried by the `formData` part of a torrent file upload.
#[derive(Debug, Deserialize)]
struct FileForm {
    torrent: Option<NewTorrent>,
}

/// Parts of a torrent file upload.
#[derive(Debug, Default)]
struct Upload {
    file: Option<Vec<u8>>,
    torrent: Option<NewTorrent>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct UpdateRequest {
    torrent_id: Option<Uuid>,
    download_client: DownloadClient,
    host_download_action: TorrentHostDownloadAction,
    category: Option<String>,
    priority: Option<i32>,
    download_retry_attempts: i32,
    torrent_retry_attempts: i32,
    delete_on_error: i32,
    lifetime: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteRequest {
    delete_data: bool,
    delete_rd_torrent: bool,
    delete_local_files: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegexRequest {
    include_regex: Option<String>,
    exclude_regex: Option<String>,
    magnet_link: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegexResult {
    include_error: String,
    exclude_error: String,
    selected_files: Vec<AvailableFile>,
}

/// Whether files matching a pattern are kept or dropped.
#[derive(Clone, Copy, Debug)]
enum Selection {
    Include,
    Exclude,
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

async fn upload_magnet(
    State(s): State<AppState>,
    value: Result<Json<MagnetRequest>, JsonRejection>,
) -> Result<(), ApiError> {
    let request = app::body(value)?;
    let magnet = request
        .magnet_link
        .as_deref()
        .filter(|link| !link.is_empty())
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "Invalid magnet link"))?;
    let torrent = request
        .torrent
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "Invalid Torrent"))?;
    s.engine
        .add_magnet(magnet, torrent.into())
        .await
        .map_err(app::internal)?;
    Ok(())
}

async fn read_upload(mut form: Multipart) -> Result<Upload, ApiError> {
    let mut upload = Upload::default();
    while let Some(field) = form.next_field().await.map_err(app::internal)? {
        match field.name() {
            Some("file") => {
                upload.file = Some(field.bytes().await.map_err(app::internal)?.to_vec());
            }
            Some("formData") => {
                let text = field.text().await.map_err(app::internal)?;
                upload.torrent = serde_json::from_str::<FileForm>(&text)
                    .map_err(app::internal)?
                    .torrent;
            }
            _ => {}
        }
    }
    Ok(upload)
}

fn torrent_file(upload: Option<Vec<u8>>) -> Result<Vec<u8>, ApiError> {
    upload
        .filter(|bytes| !bytes.is_empty())
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "Invalid torrent file"))
}

async fn upload_file(State(s): State<AppState>, form: Multipart) -> Result<(), ApiError> {
    let upload = read_upload(form).await?;
    let file = torrent_file(upload.file)?;
    let torrent = upload
        .torrent
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "Invalid Torrent"))?;
    s.engine
        .add_torrent_file(&file, torrent.into())
        .await
        .map_err(app::internal)?;
    Ok(())
}

async fn check_magnet(
    State(s): State<AppState>,
    value: Result<Json<MagnetOnly>, JsonRejection>,
) -> Result<Json<Vec<AvailableFile>>, ApiError> {
    let request = app::body(value)?;
    let magnet = request
        .magnet_link
        .as_deref()
        .filter(|link| !link.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "MagnetLink cannot be null or empty",
            )
        })?;
    let files = s
        .engine
        .available_files_for_magnet(magnet)
        .await
        .map_err(app::internal)?;
    Ok(Json(files))
}

async fn check_file(
    State(s): State<AppState>,
    form: Multipart,
) -> Result<Json<Vec<AvailableFile>>, ApiError> {
    let file = torrent_file(read_upload(form).await?.file)?;
    let files = s
        .engine
        .available_files_for_torrent(&file)
        .await
        .map_err(app::internal)?;
    Ok(Json(files))
}

/// Keep or drop the files whose name matches `pattern`; an invalid pattern selects nothing.
fn select_files(
    files: &mut Vec<AvailableFile>,
    pattern: &str,
    selection: Selection,
) -> Result<(), String> {
    match Regex::new(pattern) {
        Ok(regex) => {
            files.retain(|file| {
                let matched = regex.is_match(&file.filename).unwrap_or(false);
                match selection {
                    Selection::Include => matched,
                    Selection::Exclude => !matched,
                }
            });
            Ok(())
        }
        Err(error) => {
            files.clear();
            Err(error.to_string())
        }
    }
}

async fn verify_regex(
    State(s): State<AppState>,
    value: Result<Json<Option<RegexRequest>>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Some(request) = app::body(value)? else {
        return Ok(StatusCode::OK.into_response());
    };
    let magnet = request
        .magnet_link
        .as_deref()
        .filter(|link| !link.is_empty())
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, ""))?;
    let mut result = RegexResult {
        include_error: String::new(),
        exclude_error: String::new(),
        selected_files: s
            .engine
            .available_files_for_magnet(magnet)
            .await
            .map_err(app::internal)?,
    };
    let non_blank = |pattern: &Option<String>| {
        pattern
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    };
    if let Some(pattern) = non_blank(&request.include_regex) {
        if let Err(error) = select_files(&mut result.selected_files, &pattern, Selection::Include) {
            result.include_error = error;
        }
    } else if let Some(pattern) = non_blank(&request.exclude_regex)
        && let Err(error) = select_files(&mut result.selected_files, &pattern, Selection::Exclude)
    {
        result.exclude_error = error;
    }
    Ok(Json(result).into_response())
}

fn valid_id(value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value).map_err(|_| ApiError::new(StatusCode::NOT_FOUND, ""))
}

async fn delete(
    State(s): State<AppState>,
    Path(id): Path<String>,
    value: Result<Json<DeleteRequest>, JsonRejection>,
) -> Result<(), ApiError> {
    let id = valid_id(&id)?;
    let request = app::body(value)?;
    let options = DeleteOptions {
        data: request.delete_data,
        provider: request.delete_rd_torrent,
        local_files: request.delete_local_files,
    };
    s.engine.delete(id, options).await.map_err(app::internal)
}

async fn retry(State(s): State<AppState>, Path(id): Path<String>) -> Result<(), ApiError> {
    s.engine.retry(valid_id(&id)?).await.map_err(app::internal)
}

async fn retry_download(State(s): State<AppState>, Path(id): Path<String>) -> Result<(), ApiError> {
    s.engine
        .retry_download(valid_id(&id)?)
        .await
        .map_err(app::internal)
}

async fn update(
    State(s): State<AppState>,
    value: Result<Json<UpdateRequest>, JsonRejection>,
) -> Result<(), ApiError> {
    let request = app::body(value)?;
    let id = request
        .torrent_id
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, ""))?;
    let update = TorrentUpdate {
        download_client: request.download_client,
        host_download_action: request.host_download_action,
        category: request.category,
        priority: request.priority,
        download_retry_attempts: request.download_retry_attempts,
        torrent_retry_attempts: request.torrent_retry_attempts,
        delete_on_error: request.delete_on_error,
        lifetime: request.lifetime,
    };
    s.engine.update(id, update).await.map_err(app::internal)
}

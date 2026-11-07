//! Torrent-client HTTP endpoints for media-manager integrations.

mod binding;
mod models;
mod preferences;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use rdc_engine::{DeleteOptions, Engine, TorrentTemplate, WaitOutcome};
use rdc_model::TorrentFinishedAction;
use serde_json::{Value, json};

use crate::{
    auth::{AuthPolicy, ClientIp, CookieScope, Login, RequestUser, Sessions, require_auth},
    http::ApiError,
};
use binding::{Params, bad_request, internal};

/// All registered templates, including both qBittorrent path prefixes and the version endpoint.
pub const PATHS: &[&str] = &[
    "/version/api",
    "/api/v2/auth/login",
    "/qbittorrent/api/v2/auth/login",
    "/api/v2/auth/logout",
    "/qbittorrent/api/v2/auth/logout",
    "/api/v2/app/version",
    "/qbittorrent/api/v2/app/version",
    "/api/v2/app/webapiversion",
    "/qbittorrent/api/v2/app/webapiversion",
    "/api/v2/app/buildinfo",
    "/qbittorrent/api/v2/app/buildinfo",
    "/api/v2/app/shutdown",
    "/qbittorrent/api/v2/app/shutdown",
    "/api/v2/app/preferences",
    "/qbittorrent/api/v2/app/preferences",
    "/api/v2/app/setpreferences",
    "/qbittorrent/api/v2/app/setpreferences",
    "/api/v2/app/defaultsavepath",
    "/qbittorrent/api/v2/app/defaultsavepath",
    "/api/v2/torrents/info",
    "/qbittorrent/api/v2/torrents/info",
    "/api/v2/torrents/count",
    "/qbittorrent/api/v2/torrents/count",
    "/api/v2/torrents/files",
    "/qbittorrent/api/v2/torrents/files",
    "/api/v2/torrents/properties",
    "/qbittorrent/api/v2/torrents/properties",
    "/api/v2/torrents/pause",
    "/qbittorrent/api/v2/torrents/pause",
    "/api/v2/torrents/resume",
    "/qbittorrent/api/v2/torrents/resume",
    "/api/v2/torrents/setsharelimits",
    "/qbittorrent/api/v2/torrents/setsharelimits",
    "/api/v2/torrents/delete",
    "/qbittorrent/api/v2/torrents/delete",
    "/api/v2/torrents/add",
    "/qbittorrent/api/v2/torrents/add",
    "/api/v2/torrents/fileprio",
    "/qbittorrent/api/v2/torrents/fileprio",
    "/api/v2/torrents/setcategory",
    "/qbittorrent/api/v2/torrents/setcategory",
    "/api/v2/torrents/categories",
    "/qbittorrent/api/v2/torrents/categories",
    "/api/v2/torrents/createcategory",
    "/qbittorrent/api/v2/torrents/createcategory",
    "/api/v2/torrents/removecategories",
    "/qbittorrent/api/v2/torrents/removecategories",
    "/api/v2/torrents/createtags",
    "/qbittorrent/api/v2/torrents/createtags",
    "/api/v2/torrents/tags",
    "/qbittorrent/api/v2/torrents/tags",
    "/api/v2/torrents/setforcestart",
    "/qbittorrent/api/v2/torrents/setforcestart",
    "/api/v2/torrents/topprio",
    "/qbittorrent/api/v2/torrents/topprio",
    "/api/v2/torrents/trackers",
    "/qbittorrent/api/v2/torrents/trackers",
    "/api/v2/sync/maindata",
    "/qbittorrent/api/v2/sync/maindata",
    "/api/v2/transfer/info",
    "/qbittorrent/api/v2/transfer/info",
];

#[derive(Clone)]
struct Context {
    engine: Engine,
    sessions: Sessions,
    policy: AuthPolicy,
}

/// Creates the qBittorrent routes, applying session protection to every non-public endpoint.
/// Merge this inside `AppBuilder` to retain base-path-aware session cookies.
pub fn router<S: Clone + Send + Sync + 'static>(
    engine: Engine,
    sessions: Sessions,
    policy: AuthPolicy,
) -> Router<S> {
    let state = Context {
        engine,
        sessions,
        policy: policy.clone(),
    };
    let mut public =
        Router::<Context>::new().route("/version/api", get(version_api).post(version_api));
    let mut protected = Router::<Context>::new();
    for name in [
        "auth/login",
        "app/version",
        "app/webapiversion",
        "app/buildinfo",
    ] {
        for prefix in ["/api/v2/", "/qbittorrent/api/v2/"] {
            public = public.route(
                &format!("{prefix}{name}"),
                get(public_request).post(public_request),
            );
        }
    }
    for name in [
        "auth/logout",
        "app/shutdown",
        "app/preferences",
        "app/setpreferences",
        "app/defaultsavepath",
        "torrents/info",
        "torrents/count",
        "torrents/files",
        "torrents/properties",
        "torrents/pause",
        "torrents/resume",
        "torrents/setsharelimits",
        "torrents/delete",
        "torrents/add",
        "torrents/fileprio",
        "torrents/setcategory",
        "torrents/categories",
        "torrents/createcategory",
        "torrents/removecategories",
        "torrents/createtags",
        "torrents/tags",
        "torrents/setforcestart",
        "torrents/topprio",
        "torrents/trackers",
        "sync/maindata",
        "transfer/info",
    ] {
        for prefix in ["/api/v2/", "/qbittorrent/api/v2/"] {
            protected = protected.route(
                &format!("{prefix}{name}"),
                get(protected_request).post(protected_request),
            );
        }
    }
    public
        .merge(protected.route_layer(middleware::from_fn_with_state(policy, require_auth)))
        .with_state(state)
}

fn endpoint(request: &Request) -> &str {
    request
        .uri()
        .path()
        .split_once("/api/v2/")
        .map_or("", |(_, end)| end)
}

async fn version_api() -> impl IntoResponse {
    "20"
}

async fn public_request(
    State(state): State<Context>,
    request: Request,
) -> Result<Response, ApiError> {
    let name = endpoint(&request).to_owned();
    match name.as_str() {
        "app/version" => Ok("v4.3.2".into_response()),
        "app/webapiversion" => Ok("2.7".into_response()),
        "app/buildinfo" => Ok(Json(json!({"bitness":64,"boost":"1.75.0","libtorrent":"1.2.11.0","openssl":"1.1.1i","qt":"5.15.2","zlib":"1.2.11"})).into_response()),
        "auth/login" => {
            let scope = CookieScope::of(request.extensions(), request.headers());
            let client = ClientIp::of(request.extensions());
            let params = Params::read(request, false).await?;
            if state.policy.is_disabled() { return Ok("Ok.".into_response()); }
            let user = params.get("username").unwrap_or_default();
            let pass = params.get("password").unwrap_or_default();
            match state.sessions.login(user,pass,client,&scope).await.map_err(internal)? {
                Login::SignedIn(cookie) => Ok(([(header::SET_COOKIE,cookie)],"Ok.").into_response()),
                Login::Throttled(_) => Ok((StatusCode::FORBIDDEN,"Your IP address has been banned after too many failed authentication attempts.").into_response()),
                _ => Ok("Fails.".into_response()),
            }
        }
        _ => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

fn hashes(params: &Params) -> Result<impl Iterator<Item = &str>, ApiError> {
    Ok(params.required("hashes")?.split('|'))
}

async fn protected_request(
    State(state): State<Context>,
    request: Request,
) -> Result<Response, ApiError> {
    let name = endpoint(&request).to_owned();
    let is_get = request.method() == axum::http::Method::GET;
    if name == "torrents/add"
        && !is_get
        && !request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.starts_with("multipart/form-data")
                    || v.starts_with("application/x-www-form-urlencoded")
            })
    {
        return Err(internal("A form content type is required"));
    }
    let scope = CookieScope::of(request.extensions(), request.headers());
    let Some(user) = request.extensions().get::<RequestUser>().cloned() else {
        return Ok(StatusCode::FORBIDDEN.into_response());
    };
    let form_only = matches!(
        name.as_str(),
        "torrents/createcategory" | "torrents/removecategories"
    );
    let params = Params::read(request, form_only).await?;
    let engine = &state.engine;
    match name.as_str() {
        "auth/logout" => Ok((
            [(
                header::SET_COOKIE,
                state.sessions.logout(&scope).map_err(internal)?,
            )],
            StatusCode::OK,
        )
            .into_response()),
        "app/shutdown"
        | "app/setpreferences"
        | "torrents/setsharelimits"
        | "torrents/createtags"
        | "torrents/setforcestart" => Ok(StatusCode::OK.into_response()),
        "app/preferences" => Ok(Json(preferences::response(
            engine,
            &state
                .sessions
                .user_name(&user)
                .await
                .map_err(internal)?
                .unwrap_or_default(),
        ))
        .into_response()),
        "app/defaultsavepath" => Ok(models::save_path(&engine.settings()).into_response()),
        "torrents/info" | "torrents/count" => {
            let settings = engine.settings();
            let torrents = engine.torrents().await.map_err(internal)?;
            let hashes: BTreeSet<String> = params
                .get("hashes")
                .unwrap_or_default()
                .split('|')
                .filter(|v| !v.is_empty())
                .map(str::to_ascii_lowercase)
                .collect();
            let infos: Vec<Value> = torrents
                .iter()
                .enumerate()
                .map(|(index, torrent)| models::info(torrent, engine, &settings, index + 1))
                .filter(|item| models::matches_filter(item, params.get("filter")))
                .filter(|item| {
                    name == "torrents/count"
                        || params
                            .get("category")
                            .filter(|v| !v.trim().is_empty())
                            .is_none_or(|value| item["category"] == value)
                })
                .filter(|item| {
                    name == "torrents/count"
                        || hashes.is_empty()
                        || item["hash"]
                            .as_str()
                            .is_some_and(|hash| hashes.contains(&hash.to_ascii_lowercase()))
                })
                .collect();
            if name == "torrents/count" {
                Ok(Json(json!(infos.len())).into_response())
            } else {
                Ok(Json(infos).into_response())
            }
        }
        "torrents/files" | "torrents/properties" | "torrents/trackers" => {
            let hash = params.required("hash")?;
            let torrent = engine.torrent_by_hash(hash).await.map_err(internal)?;
            let Some(torrent) = torrent else {
                return if name == "torrents/trackers" {
                    Ok(Json(json!([])).into_response())
                } else {
                    Ok(StatusCode::NOT_FOUND.into_response())
                };
            };
            let value = match name.as_str() {
                "torrents/files" => models::files(&torrent),
                "torrents/properties" => models::properties(&torrent, engine, &engine.settings()),
                _ => {
                    let kind = torrent
                        .client_kind
                        .map_or_else(String::new, |kind| kind.to_string());
                    let host = torrent.rd_host.as_deref().unwrap_or(&kind);
                    json!([{"url":format!("http://{host}/**").to_lowercase(),
                        "status":"Working","num_peers":torrent.rd_seeders.unwrap_or(1),
                        "msg":format!("Fake Tracker for {kind}")}])
                }
            };
            Ok(Json(value).into_response())
        }
        "torrents/pause"
        | "torrents/resume"
        | "torrents/topprio"
        | "torrents/delete"
        | "torrents/setcategory" => {
            let delete_files = if name == "torrents/delete" {
                params.boolean("deletefiles")?
            } else {
                false
            };
            for hash in hashes(&params)? {
                match name.as_str() {
                    "torrents/pause" => engine.pause(hash).await.map_err(internal)?,
                    "torrents/resume" => engine.resume(hash).await.map_err(internal)?,
                    "torrents/topprio" => {
                        engine.set_priority(hash, Some(1)).await.map_err(internal)?;
                    }
                    "torrents/setcategory" => engine
                        .set_category(
                            hash,
                            Some(params.get("category").unwrap_or_default().to_owned()),
                        )
                        .await
                        .map_err(internal)?,
                    _ => {
                        if let Some(torrent) =
                            engine.torrent_by_hash(hash).await.map_err(internal)?
                        {
                            let options =
                                match engine.settings().integrations.default.finished_action {
                                    TorrentFinishedAction::None => continue,
                                    TorrentFinishedAction::RemoveAllTorrents => DeleteOptions {
                                        data: true,
                                        provider: true,
                                        local_files: delete_files,
                                    },
                                    TorrentFinishedAction::RemoveProvider => DeleteOptions {
                                        data: false,
                                        provider: true,
                                        local_files: delete_files,
                                    },
                                    TorrentFinishedAction::RemoveClient => DeleteOptions {
                                        data: true,
                                        provider: false,
                                        local_files: delete_files,
                                    },
                                };
                            engine
                                .delete(torrent.torrent_id, options)
                                .await
                                .map_err(internal)?;
                        }
                    }
                }
            }
            Ok(StatusCode::OK.into_response())
        }
        "torrents/add" => add(engine, params, is_get).await,
        "torrents/fileprio" => {
            let hash = params.required("hash")?;
            let ids = params
                .required("id")?
                .split('|')
                .filter_map(|v| v.parse::<usize>().ok())
                .collect::<Vec<_>>();
            if ids.is_empty() {
                return Err(bad_request("id must contain an integer"));
            }
            let priority = params
                .integer("priority")?
                .ok_or_else(|| bad_request("priority is required"))?;
            engine
                .update_file_selection(hash, &ids, priority > 0)
                .await
                .map_err(internal)?;
            Ok(StatusCode::OK.into_response())
        }
        "torrents/categories" => Ok(Json(models::categories(
            &engine.torrents().await.map_err(internal)?,
            &engine.settings(),
        ))
        .into_response()),
        "torrents/createcategory" => {
            let name = params.get("category").unwrap_or_default().trim();
            if name.is_empty() {
                return Err(bad_request("category name is empty"));
            }
            let settings = engine.settings();
            let mut categories = settings
                .general
                .categories
                .as_deref()
                .unwrap_or_default()
                .split(',')
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if !categories.iter().any(|value| value == name) {
                categories.push(name.to_owned());
            }
            engine
                .update_setting("General:Categories", Some(&categories.join(",")))
                .await
                .map_err(internal)?;
            Ok(StatusCode::OK.into_response())
        }
        "torrents/removecategories" => {
            let names: BTreeSet<_> = params
                .get("categories")
                .unwrap_or_default()
                .split('\n')
                .map(str::trim)
                .collect();
            if names.iter().any(|name| !name.is_empty()) {
                let settings = engine.settings();
                let categories = settings
                    .general
                    .categories
                    .as_deref()
                    .unwrap_or_default()
                    .split(',')
                    .filter(|v| !v.is_empty() && !names.contains(v))
                    .collect::<Vec<_>>()
                    .join(",");
                engine
                    .update_setting("General:Categories", Some(&categories))
                    .await
                    .map_err(internal)?;
            }
            Ok(StatusCode::OK.into_response())
        }
        "torrents/tags" => Ok(Json(json!([])).into_response()),
        "sync/maindata" => {
            let torrents = engine.torrents().await.map_err(internal)?;
            Ok(Json(models::sync(&torrents, engine, &engine.settings())).into_response())
        }
        "transfer/info" => Ok(Json(models::transfer(engine)).into_response()),
        _ => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

async fn add(engine: &Engine, params: Params, is_get: bool) -> Result<Response, ApiError> {
    let mut template = TorrentTemplate::from_defaults(
        &engine.settings().integrations.default,
        engine.settings().download_client.client,
    );
    template.finished_action = TorrentFinishedAction::None;
    if let Some(category) = params.get("category") {
        template.category = Some(category.to_owned());
    }
    if let Some(priority) = params.integer("priority")? {
        template.priority = Some(priority);
    }
    for bytes in &params.files {
        let torrent = engine
            .add_torrent_file(bytes, template.clone())
            .await
            .map_err(internal)?;
        if !wait(engine, torrent.torrent_id).await? {
            return Ok("Fails.".into_response());
        }
    }
    if let Some(urls) = params.get("urls") {
        if urls.trim().is_empty() {
            return Err(bad_request("urls is required"));
        }
        for raw in urls.split('\n') {
            let url = raw.trim();
            let torrent = if url.starts_with("magnet") {
                engine
                    .add_magnet(url, template.clone())
                    .await
                    .map_err(internal)?
            } else if url.starts_with("http") {
                let bytes = reqwest::get(url)
                    .await
                    .map_err(internal)?
                    .error_for_status()
                    .map_err(internal)?
                    .bytes()
                    .await
                    .map_err(internal)?;
                engine
                    .add_torrent_file(&bytes, template.clone())
                    .await
                    .map_err(internal)?
            } else {
                return Err(bad_request(format!("Invalid torrent link format {url}")));
            };
            if !wait(engine, torrent.torrent_id).await? {
                return Ok("Fails.".into_response());
            }
        }
        return Ok("Ok.".into_response());
    }
    if is_get {
        Err(bad_request("urls is required"))
    } else {
        Ok(StatusCode::OK.into_response())
    }
}

async fn wait(engine: &Engine, id: uuid::Uuid) -> Result<bool, ApiError> {
    let result = engine
        .wait_until_dequeued(id, models::DEQUEUE_TIMEOUT)
        .await
        .map_err(internal)?;
    match result {
        WaitOutcome::TimedOut => Ok(true),
        WaitOutcome::Missing | WaitOutcome::Shutdown => Ok(false),
        WaitOutcome::Dequeued | WaitOutcome::Completed => {
            let torrent = engine.torrent(id).await.map_err(internal)?;
            Ok(torrent.is_some_and(|torrent| {
                torrent.error.is_none()
                    && torrent.rd_status != Some(rdc_model::TorrentStatus::Error)
            }))
        }
    }
}

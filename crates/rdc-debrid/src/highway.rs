//! Client for the HIGHWAY cloud download center (`high-way.me`).
//!
//! The whole API is one endpoint, `transfers.php`, selected by query
//! parameters. Only torrent transfers are managed.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use async_trait::async_trait;
use jiff::Timestamp;
use rdc_model::{
    Download, DownloadClient, DownloadInfo, Provider, RemoteFile, Torrent, TorrentStatus,
};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::provider::{AvailableFile, DebridProvider, DebridUser, ProviderConfig, RemoteTorrent};
use crate::text_rate_limit::classify_add_error;
use crate::values::{base_name, non_blank};

/// The public HIGHWAY API endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://cloud.high-way.me/center-api/transfers.php";

/// The only service rd-client uses.
const SERVICE: &str = "torrent";

/// Account name reported when the server has no statistics.
const FALLBACK_USERNAME: &str = "HIGHWAY";

/// Top-level folder of torrent transfers in the shared file tree.
const SERVICE_DIR: &str = "TORRENT";

/// Scheme of the links rd-client hands out for transfers without signed
/// per-file URLs; resolved by [`Highway::unrestrict`].
const LINK_SCHEME: &str = "highway://";

/// `rd_status_raw` of a transfer the provider no longer knows.
const DELETED: &str = "deleted";

/// Whether `id` is a spool reference: the 32-hex id of a queued transfer
/// that has no numeric transfer id yet.
pub fn is_spool_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Client for a HIGHWAY account.
#[derive(Clone)]
pub struct Highway {
    http: ProviderHttp,
    api_key: String,
    endpoint: String,
}

impl fmt::Debug for Highway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Highway")
            .field("api_key", &"<redacted>")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Highway {
    /// Creates a client for the account of the API token `config.api_key`.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self {
            http,
            api_key: config.api_key.trim().to_owned(),
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        }
    }

    /// Sends requests to `endpoint` instead of the public API.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        endpoint.clone_into(&mut self.endpoint);
        self
    }

    /// Lists the torrent transfers of the account (other services are skipped).
    pub async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        let list: Option<TransferList> = self
            .call(Method::GET, &[])
            .await?
            .as_deref()
            .map(decode)
            .transpose()?;
        Ok(list
            .and_then(|list| list.transfers)
            .unwrap_or_default()
            .into_iter()
            .filter(|transfer| transfer.service.as_deref() != Some("usenet"))
            .map(Transfer::into_remote)
            .collect())
    }

    /// Returns the account from the server statistics.
    ///
    /// Servers without statistics (or answering with something else) yield
    /// a stub account named `HIGHWAY`.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let stats: Stats = self
            .call(Method::GET, &[("stats", "1")])
            .await?
            .and_then(|body| serde_json::from_str(&body).ok())
            .unwrap_or_default();
        let username = non_blank(stats.username.as_deref())
            .or_else(|| non_blank(stats.plan.as_deref()))
            .unwrap_or(FALLBACK_USERNAME)
            .to_owned();
        Ok(DebridUser {
            username: Some(username),
            expiration: stats
                .plan_expires_at
                .and_then(|seconds| Timestamp::from_second(seconds).ok()),
            premium: stats.premium.unwrap_or(false),
        })
    }

    /// Adds a magnet link and returns the transfer id (or spool id).
    pub async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError> {
        self.add(Form::new().text("magnet", magnet.to_owned()))
            .await
    }

    /// Uploads a `.torrent` file and returns the transfer id (or spool id).
    pub async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError> {
        let file = Part::bytes(bytes.to_vec())
            .file_name("file.torrent")
            .mime_str("application/octet-stream")?;
        self.add(Form::new().part("file", file)).await
    }

    async fn add(&self, form: Form) -> Result<String, DebridError> {
        let request = self.request(Method::POST, &[])?.multipart(form);
        let body = self.send(request).await.map_err(classify_add_error)?;
        let transfer: Option<Transfer> = body.as_deref().map(decode).transpose()?;
        transfer
            .and_then(|transfer| transfer.id)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                DebridError::InvalidResponse("HIGHWAY API did not return a transfer id.".to_owned())
            })
    }

    /// Removes the transfer; a torrent without provider id is left alone.
    pub async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError> {
        let Some(id) = non_blank(torrent.rd_id.as_deref()) else {
            return Ok(());
        };
        self.call(Method::DELETE, &id_query(id, service_for(id)))
            .await?;
        Ok(())
    }

    /// Copies the transfer state into the torrent's `rd_*` fields.
    ///
    /// Uses `remote` when given and otherwise looks the transfer up; a
    /// transfer the server no longer knows gets `rd_status_raw = "deleted"`.
    /// A failed transfer stores its detail in [`Torrent::error`].
    pub async fn refresh(
        &self,
        torrent: &mut Torrent,
        remote: Option<&RemoteTorrent>,
    ) -> Result<(), DebridError> {
        let Some(id) = non_blank(torrent.rd_id.as_deref()) else {
            return Ok(());
        };
        let fetched;
        let remote = match remote {
            Some(remote) => remote,
            None => match self.transfer(id, service_for(id)).await {
                Ok(Some(transfer)) => {
                    fetched = transfer.into_remote();
                    &fetched
                }
                Ok(None) => {
                    torrent.rd_status_raw = Some(DELETED.to_owned());
                    return Ok(());
                }
                Err(error) => {
                    tracing::error!("failed to update HIGHWAY transfer {id}: {error}");
                    return Err(error);
                }
            },
        };
        apply(torrent, remote);
        Ok(())
    }

    /// Resolves a `highway://<id>/<service>/zip` link to the signed zip URL of
    /// the transfer; every other link (real URLs, per-file references) is
    /// returned unchanged.
    pub async fn unrestrict(&self, link: &str) -> Result<String, DebridError> {
        if link.trim().is_empty() {
            return Err(DebridError::Provider(
                "The download link is empty".to_owned(),
            ));
        }
        let Some(reference) = link.strip_prefix(LINK_SCHEME) else {
            return Ok(link.to_owned());
        };
        let parts: Vec<&str> = reference.split('/').collect();
        let &[id, service, file_ref] = parts.as_slice() else {
            return Err(DebridError::Provider(format!(
                "Invalid HIGHWAY link format: {link}"
            )));
        };
        if file_ref != "zip" {
            return Ok(link.to_owned());
        }
        let transfer = self
            .transfer(id, non_blank(Some(service)))
            .await?
            .ok_or_else(|| {
                DebridError::NotFound(format!(
                    "HIGHWAY transfer {id} was not found. It was likely deleted or unlinked on \
                     HIGHWAY since this torrent was added here - remove it here and re-add it."
                ))
            })?;
        non_blank(transfer.download_url.as_deref())
            .map(str::to_owned)
            .ok_or_else(|| {
                DebridError::Provider(format!(
                    "HIGHWAY did not return a download URL for transfer {id}. Signed URLs may be \
                     disabled on the server; use the Symlink download client with an rclone \
                     WebDAV mount instead."
                ))
            })
    }

    /// Lists the files of a ready cached transfer for the info hash `hash`
    /// (40 or 64 hex characters), when this account has it linked.
    pub async fn available_files(&self, hash: &str) -> Result<Vec<AvailableFile>, DebridError> {
        if !matches!(hash.len(), 40 | 64) || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(Vec::new());
        }
        // Servers without a cache lookup answer with something else.
        let check: Option<CacheCheck> = self
            .call(Method::GET, &[("check", hash), ("service", SERVICE)])
            .await?
            .and_then(|body| serde_json::from_str(&body).ok());
        let Some(entry) = check
            .and_then(|check| check.results)
            .and_then(|mut results| results.remove(hash))
            .filter(|entry| entry.ready)
        else {
            return Ok(Vec::new());
        };
        let Some(id) = non_blank(entry.id.as_deref()) else {
            return Ok(Vec::new());
        };
        let transfer = self.transfer(id, Some(SERVICE)).await?;
        Ok(transfer
            .and_then(|transfer| transfer.files)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|file| {
                let filename = file.path.filter(|path| !path.trim().is_empty())?;
                Some(AvailableFile {
                    filename,
                    filesize: file.size,
                })
            })
            .collect())
    }

    async fn transfer(
        &self,
        id: &str,
        service: Option<&str>,
    ) -> Result<Option<Transfer>, DebridError> {
        self.call(Method::GET, &id_query(id, service))
            .await?
            .as_deref()
            .map(decode)
            .transpose()
    }

    /// Sends a request built by [`request`](Self::request) and returns its
    /// body; `None` for 404 and empty bodies.
    async fn call(
        &self,
        method: Method,
        query: &[(&str, &str)],
    ) -> Result<Option<String>, DebridError> {
        self.send(self.request(method, query)?).await
    }

    fn request(
        &self,
        method: Method,
        query: &[(&str, &str)],
    ) -> Result<RequestBuilder, DebridError> {
        if self.api_key.trim().is_empty() {
            return Err(DebridError::NotConfigured(
                "The HIGHWAY API token is not set (provider API key)".to_owned(),
            ));
        }
        Ok(self
            .http
            .client()
            .request(method, &self.endpoint)
            .query(query)
            .bearer_auth(&self.api_key))
    }

    /// Sends a request; `None` for 404 and empty bodies.
    async fn send(&self, request: RequestBuilder) -> Result<Option<String>, DebridError> {
        let response = self.http.send(request).await.inspect_err(|error| {
            if let DebridError::Transport(source) = error
                && source.is_timeout()
            {
                tracing::error!("the connection to HIGHWAY timed out: {source}");
            }
        })?;
        let status = response.status();
        let body = response.text().await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(error_from(status, &body));
        }
        Ok((!body.trim().is_empty()).then_some(body))
    }
}

#[async_trait]
impl DebridProvider for Highway {
    fn kind(&self) -> Provider {
        Provider::Highway
    }

    async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        Highway::list_torrents(self).await
    }

    async fn user(&self) -> Result<DebridUser, DebridError> {
        Highway::user(self).await
    }

    async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError> {
        Highway::add_magnet(self, magnet).await
    }

    async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError> {
        Highway::add_torrent_file(self, bytes).await
    }

    async fn available_files(&self, hash: &str) -> Result<Vec<AvailableFile>, DebridError> {
        Highway::available_files(self, hash).await
    }

    /// Transfers always download every file. The symlink downloader needs
    /// the file list, so it waits until the server indexed it; the other
    /// downloaders can fall back to the zip of the whole transfer.
    async fn select_files(&self, torrent: &Torrent) -> Result<Option<usize>, DebridError> {
        let count = torrent.files().len();
        if count == 0 && torrent.download_client == DownloadClient::Symlink {
            return Ok(None);
        }
        Ok(Some(count.max(1)))
    }

    async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError> {
        Highway::delete(self, torrent).await
    }

    async fn unrestrict(&self, _torrent: &Torrent, link: &str) -> Result<String, DebridError> {
        Highway::unrestrict(self, link).await
    }

    async fn refresh(
        &self,
        torrent: &mut Torrent,
        remote: Option<&RemoteTorrent>,
    ) -> Result<(), DebridError> {
        Highway::refresh(self, torrent, remote).await
    }

    /// Works from the stored file list, without a request.
    ///
    /// The symlink downloader gets one `highway://<id>/torrent/<file id>`
    /// reference per file; the other downloaders get the signed per-file
    /// URLs when every file has one and the `highway://<id>/torrent/zip`
    /// reference otherwise. Spool references and transfers without an
    /// indexed file list are not ready yet.
    async fn download_infos(
        &self,
        torrent: &Torrent,
    ) -> Result<Option<Vec<DownloadInfo>>, DebridError> {
        download_infos(torrent)
    }

    async fn file_name(&self, download: &Download) -> Result<String, DebridError> {
        non_blank(download.file_name.as_deref())
            .map(str::to_owned)
            .or_else(|| rdc_model::paths::file_name(download))
            .ok_or_else(|| {
                DebridError::InvalidResponse(format!(
                    "No file name is known for the download of {}",
                    download.path
                ))
            })
    }

    /// The path inside the mount of the shared file tree:
    /// `TORRENT/<file path>`, e.g. `TORRENT/Some.Release/Some.Release.mkv`.
    fn symlink_path(
        &self,
        torrent: &Torrent,
        download: &Download,
    ) -> Result<Option<String>, DebridError> {
        Ok(symlink_path(torrent, download))
    }
}

fn download_infos(torrent: &Torrent) -> Result<Option<Vec<DownloadInfo>>, DebridError> {
    let Some(id) = non_blank(torrent.rd_id.as_deref()).filter(|id| !is_spool_id(id)) else {
        return Ok(None);
    };
    let files = torrent.files();
    if files.is_empty() {
        return Ok(None);
    }
    let filter = torrent.file_filter()?;
    let mut downloadable = Vec::new();
    for file in files {
        if file.selected && filter.is_downloadable(&file.path, file.bytes)? {
            downloadable.push(file);
        }
    }
    let info = |file: &RemoteFile, restricted_link: String| DownloadInfo {
        file_name: Some(base_name(&file.path).to_owned()),
        restricted_link,
    };
    if torrent.download_client == DownloadClient::Symlink {
        let infos = downloadable
            .iter()
            .map(|file| info(file, format!("{LINK_SCHEME}{id}/{SERVICE}/{}", file.id)))
            .collect();
        return Ok(Some(infos));
    }
    if downloadable.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let signed: Option<Vec<DownloadInfo>> = downloadable
        .iter()
        .map(|file| {
            let link = non_blank(file.download_link.as_deref())?;
            Some(info(file, link.to_owned()))
        })
        .collect();
    if let Some(infos) = signed {
        return Ok(Some(infos));
    }
    let name = torrent.rd_name.as_deref().unwrap_or(id);
    Ok(Some(vec![DownloadInfo {
        file_name: Some(format!("{name}.zip")),
        restricted_link: format!("{LINK_SCHEME}{id}/{SERVICE}/zip"),
    }]))
}

fn symlink_path(torrent: &Torrent, download: &Download) -> Option<String> {
    let file_name = rdc_model::paths::file_name(download).filter(|name| !name.trim().is_empty())?;
    let files = torrent.files();
    let relative = files
        .iter()
        .find(|file| !file.path.trim().is_empty() && file.path.ends_with(&file_name))
        .map_or(file_name.as_str(), |file| file.path.as_str());
    let service_dir = torrent
        .rd_host
        .as_deref()
        .and_then(|host| {
            host.split(['/', '\\'])
                .find(|segment| !segment.trim().is_empty())
        })
        .unwrap_or(SERVICE_DIR);
    if starts_with_dir(relative, service_dir) {
        return Some(relative.to_owned());
    }
    Some(
        Path::new(service_dir)
            .join(relative)
            .to_string_lossy()
            .into_owned(),
    )
}

/// Whether `path` starts with the directory `dir` (compared case-insensitively).
fn starts_with_dir(path: &str, dir: &str) -> bool {
    path.get(..dir.len())
        .is_some_and(|prefix| prefix.to_lowercase() == dir.to_lowercase())
        && matches!(path.as_bytes().get(dir.len()), Some(b'/' | b'\\'))
}

/// The service to name for `id`: spool references are looked up without one.
fn service_for(id: &str) -> Option<&'static str> {
    (!is_spool_id(id)).then_some(SERVICE)
}

fn id_query<'a>(id: &'a str, service: Option<&'a str>) -> Vec<(&'static str, &'a str)> {
    let mut query = vec![("id", id)];
    if let Some(service) = service {
        query.push(("service", service));
    }
    query
}

fn apply(torrent: &mut Torrent, remote: &RemoteTorrent) {
    if !remote.filename.trim().is_empty() {
        torrent.rd_name = Some(remote.filename.clone());
    }
    if remote.bytes > 0 {
        torrent.rd_size = Some(remote.bytes);
    }
    if let Some(files) = &remote.files {
        torrent.set_files(files);
    }
    torrent.client_kind = Some(Provider::Highway);
    torrent.rd_host.clone_from(&remote.host);
    torrent.rd_progress = Some(remote.progress);
    torrent.rd_added.get_or_insert_with(Timestamp::now);
    torrent.rd_speed = remote.speed;
    torrent.rd_seeders = remote.seeders;
    torrent.rd_status_raw.clone_from(&remote.status);
    if torrent.rd_ended.is_none() {
        torrent.rd_ended = remote.ended;
    }
    let state = remote.status.as_deref();
    torrent.rd_status = Some(match state {
        Some("queued") => TorrentStatus::Processing,
        Some("downloading") => TorrentStatus::Downloading,
        Some("completed") => TorrentStatus::Finished,
        Some("error") => TorrentStatus::Error,
        _ => match torrent.rd_status {
            None | Some(TorrentStatus::Queued) => TorrentStatus::Processing,
            Some(previous) => previous,
        },
    });
    if state == Some("error")
        && let Some(detail) = non_blank(remote.message.as_deref())
    {
        torrent.error = Some(detail.to_owned());
    }
}

fn decode<T: DeserializeOwned>(body: &str) -> Result<T, DebridError> {
    serde_json::from_str(body).map_err(|error| {
        DebridError::InvalidResponse(format!("HIGHWAY returned an unreadable response: {error}"))
    })
}

/// Error body of a failed request.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ApiError {
    error: Option<String>,
    message: Option<String>,
}

fn error_from(status: StatusCode, body: &str) -> DebridError {
    let ApiError { error, message } = serde_json::from_str(body).unwrap_or_default();
    let code = error.unwrap_or_default();
    let with_message = |text: String| match message.as_deref() {
        Some(message) if !message.trim().is_empty() => format!("{text} {message}"),
        _ => text,
    };
    match code.as_str() {
        "invalid_token" | "authentication_required" => {
            DebridError::NotConfigured(with_message(format!(
                "HIGHWAY authentication failed ({code}). Check the API token in the provider settings."
            )))
        }
        "system_feature_unavailable" => DebridError::NotConfigured(with_message(format!(
            "The HIGHWAY token does not belong to a high-way.me account ({code})."
        ))),
        _ => {
            let label = if code.is_empty() {
                status.as_u16().to_string()
            } else {
                format!("{} {code}", status.as_u16())
            };
            let detail = message.unwrap_or_else(|| body.trim().to_owned());
            DebridError::Provider(format!("HIGHWAY request failed ({label}): {detail}"))
        }
    }
}

/// Answer of `stats=1`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Stats {
    username: Option<String>,
    plan: Option<String>,
    /// Unix seconds; `None` for plans without expiry.
    plan_expires_at: Option<i64>,
    premium: Option<bool>,
}

/// Reads an id sent either as a string or as a number.
fn id_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Text(String),
        Number(i64),
    }
    Ok(Option::<Id>::deserialize(deserializer)?.map(|id| match id {
        Id::Text(text) => text,
        Id::Number(number) => number.to_string(),
    }))
}

/// Answer of the transfer listing.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TransferList {
    transfers: Option<Vec<Transfer>>,
}

/// One transfer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Transfer {
    #[serde(deserialize_with = "id_text")]
    id: Option<String>,
    /// `torrent` or `usenet`.
    service: Option<String>,
    name: Option<String>,
    /// `queued`, `downloading`, `completed`, `error`, ...
    state: Option<String>,
    state_detail: Option<String>,
    /// Percent.
    progress: i64,
    size: i64,
    downloaded: i64,
    /// Seconds left.
    eta: Option<i64>,
    seeder: i64,
    /// Location in the shared file tree, e.g. `TORRENT/<name>`.
    content_path: Option<String>,
    /// Signed link to the whole transfer as a zip.
    download_url: Option<String>,
    files: Option<Vec<TransferFile>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TransferFile {
    path: Option<String>,
    size: i64,
    partial: bool,
    /// Signed link to this file, when the server signs file URLs.
    download_url: Option<String>,
}

impl Transfer {
    /// Maps the transfer; it has no info hash, so the id stands in for it.
    /// `added` is now, `ended` is now once completed.
    fn into_remote(self) -> RemoteTorrent {
        let now = Timestamp::now();
        let speed = self
            .eta
            .filter(|&eta| eta > 0 && self.size > self.downloaded && self.downloaded >= 0)
            .map(|eta| (self.size - self.downloaded) / eta);
        let files = self
            .files
            .unwrap_or_default()
            .into_iter()
            .zip(0_i64..)
            .map(|(file, id)| RemoteFile {
                id,
                path: file.path.unwrap_or_default(),
                bytes: file.size,
                selected: !file.partial,
                download_link: file.download_url,
            })
            .collect();
        let id = self.id.unwrap_or_default();
        RemoteTorrent {
            hash: id.clone(),
            id,
            filename: self.name.clone().unwrap_or_default(),
            original_filename: self.name,
            bytes: self.size,
            original_bytes: self.size,
            host: self.content_path,
            progress: self.progress,
            ended: (self.state.as_deref() == Some("completed")).then_some(now),
            status: self.state,
            message: self.state_detail,
            added: Some(now),
            files: Some(files),
            links: Some(Vec::new()),
            speed,
            seeders: Some(self.seeder),
            ..RemoteTorrent::default()
        }
    }
}

/// Answer of `check=<hash>`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CacheCheck {
    results: Option<HashMap<String, CacheEntry>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CacheEntry {
    ready: bool,
    /// Transfer id, when this account already linked the cached transfer.
    #[serde(deserialize_with = "id_text")]
    id: Option<String>,
}

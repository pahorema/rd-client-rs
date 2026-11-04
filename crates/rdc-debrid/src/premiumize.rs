//! Premiumize.me client.
//!
//! Every call authenticates with the `apikey` query parameter. Responses are
//! JSON objects whose `status` is `"success"` unless the call failed, in which
//! case `message` explains why.
//!
//! The transfer listing reports neither sizes nor files, so a refreshed
//! torrent keeps no size and an empty file list; the files to download are
//! discovered from the transfer's folder when downloads are created.

use std::fmt;
use std::pin::Pin;
use std::time::Duration;

use jiff::Timestamp;
use rdc_model::{DownloadInfo, FileFilter, Provider, Torrent, TorrentStatus};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::{DeserializeOwned, IgnoredAny};

use crate::error::DebridError;
use crate::http::{DEFAULT_RATE_LIMIT_COOLDOWN, ProviderHttp};
use crate::provider::{DebridUser, ProviderConfig, RemoteTorrent};

/// Address of the Premiumize API.
pub const DEFAULT_BASE_URL: &str = "https://www.premiumize.me/api/";

/// Fragments of Premiumize error messages and codes that mean "slow down".
const RATE_LIMIT_MARKERS: [&str; 14] = [
    "slow_down",
    "rate limit exceeded",
    "rate_limit_reached",
    "account_limit_reached",
    "service_limit_reached",
    "service_down",
    "semi_permanent_error",
    "too many api requests",
    "fair-use points",
    "booster points",
    "active-job count",
    "usage limit for this service",
    "target service is unreachable",
    "retry after a delay",
];

/// Longest part of an unexpected response body quoted in error messages.
const BODY_EXCERPT_CHARS: usize = 300;

/// Client for a Premiumize.me account.
#[derive(Clone)]
pub struct Premiumize {
    http: ProviderHttp,
    base_url: String,
    api_key: String,
    timeout: Duration,
}

impl fmt::Debug for Premiumize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Premiumize")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Premiumize {
    /// Creates a client for the account of `config.api_key`.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self {
            http,
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key: config.api_key.clone(),
            timeout: config.timeout,
        }
    }

    /// Sends requests to `base_url` instead of [`DEFAULT_BASE_URL`].
    #[must_use]
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = if base_url.ends_with('/') {
            base_url.to_owned()
        } else {
            format!("{base_url}/")
        };
        self
    }

    /// Returns the account the API key belongs to.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let account: Account = self
            .call(self.request(Method::GET, "account/info", &[])?)
            .await?;
        let expiration = account
            .premium_until
            .as_ref()
            .and_then(serde_json::Value::as_i64)
            .filter(|seconds| *seconds > 0)
            .and_then(|seconds| Timestamp::from_second(seconds).ok());
        Ok(DebridUser {
            username: Some(account.customer_id.to_string()),
            expiration,
            premium: false,
        })
    }

    /// Lists every transfer of the account.
    pub async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        Ok(self
            .transfers()
            .await?
            .iter()
            .map(Transfer::to_remote)
            .collect())
    }

    /// Adds a magnet link and returns the id of the new transfer.
    pub async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError> {
        let request = self
            .request(Method::POST, "transfer/create", &[])?
            .form(&[("src", magnet), ("folder_id", "")]);
        created_id(self.call(request).await?, "magnet link")
    }

    /// Uploads a `.torrent` file and returns the id of the new transfer.
    pub async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError> {
        let file = Part::bytes(bytes.to_vec())
            .file_name("1.torrent")
            .mime_str("application/x-bittorrent")?;
        let form = Form::new().part("file", file).text("folder_id", "");
        let request = self
            .request(Method::POST, "transfer/create", &[])?
            .multipart(form);
        created_id(self.call(request).await?, "torrent file")
    }

    /// Removes the transfer `id`.
    pub async fn delete_transfer(&self, id: &str) -> Result<(), DebridError> {
        let request = self
            .request(Method::POST, "transfer/delete", &[])?
            .form(&[("id", id)]);
        let _: IgnoredAny = self.call(request).await?;
        Ok(())
    }

    /// Copies the transfer state into the torrent's `rd_*` fields.
    ///
    /// Without `remote` the transfer is looked up in the listing; a transfer
    /// that is no longer listed marks the torrent `"deleted"`.
    pub async fn refresh(
        &self,
        torrent: &mut Torrent,
        remote: Option<&RemoteTorrent>,
    ) -> Result<(), DebridError> {
        let Some(rd_id) = torrent.rd_id.as_deref() else {
            return Ok(());
        };
        let fetched;
        let remote = match remote {
            Some(remote) => remote,
            None => match self.transfer(rd_id).await {
                Ok(transfer) => {
                    fetched = transfer.to_remote();
                    &fetched
                }
                Err(error) if error.is_not_found() => {
                    tracing::info!(rd_id, "Premiumize transfer no longer exists");
                    torrent.rd_status_raw = Some("deleted".to_owned());
                    return Ok(());
                }
                Err(error) => return Err(error),
            },
        };
        apply(torrent, remote);
        Ok(())
    }

    /// Lists the files of the torrent's transfer that pass its file filter.
    ///
    /// Files come from the transfer folder, searched recursively; a
    /// sub-folder is entered only when its name passes the filter's path
    /// rules. A single-file transfer adds its file without filtering.
    /// Returns `None` when the torrent has not been sent to Premiumize yet.
    pub async fn download_infos(
        &self,
        torrent: &Torrent,
    ) -> Result<Option<Vec<DownloadInfo>>, DebridError> {
        let Some(rd_id) = torrent.rd_id.as_deref() else {
            return Ok(None);
        };
        let transfer = self
            .transfers()
            .await?
            .into_iter()
            .find(|transfer| transfer.id == rd_id)
            .ok_or_else(|| DebridError::NotFound(format!("Transfer {rd_id} not found")))?;
        let filter = torrent.file_filter()?;
        let mut infos = match non_blank(transfer.folder_id.as_deref()) {
            Some(folder_id) => self.folder_infos(&filter, folder_id).await?,
            None => Vec::new(),
        };
        if let Some(file_id) = non_blank(transfer.file_id.as_deref()) {
            let item: Item = self
                .call(self.request(Method::GET, "item/details", &[("id", file_id)])?)
                .await?;
            if non_blank(item.link.as_deref()).is_none() {
                tracing::debug!(file_id, "Premiumize file has no link");
            }
            infos.push(DownloadInfo {
                file_name: item.name,
                restricted_link: item.link.unwrap_or_default(),
            });
        }
        Ok(Some(infos))
    }

    fn folder_infos<'a>(&'a self, filter: &'a FileFilter, folder_id: &'a str) -> FolderInfos<'a> {
        Box::pin(async move {
            let folder: Folder = self
                .call(self.request(
                    Method::GET,
                    "folder/list",
                    &[("includebreadcrumbs", "true"), ("id", folder_id)],
                )?)
                .await?;
            let mut infos = Vec::new();
            for item in folder.content.unwrap_or_default() {
                let name = item.name.as_deref().unwrap_or_default();
                match item.kind.as_deref() {
                    Some("file") => {
                        let Some(link) = non_blank(item.link.as_deref()) else {
                            tracing::debug!(folder_id, name, "Premiumize file has no link");
                            continue;
                        };
                        if filter.is_downloadable(name, item.size.unwrap_or_default())? {
                            infos.push(DownloadInfo {
                                file_name: item.name.clone(),
                                restricted_link: link.to_owned(),
                            });
                        }
                    }
                    // Folders have no size: only the path rules apply to them.
                    Some("folder") => {
                        if let Some(id) = non_blank(item.id.as_deref())
                            && filter.is_downloadable(name, i64::MAX)?
                        {
                            infos.extend(self.folder_infos(filter, id).await?);
                        }
                    }
                    kind => tracing::debug!(folder_id, name, ?kind, "skipping Premiumize item"),
                }
            }
            Ok(infos)
        })
    }

    async fn transfer(&self, id: &str) -> Result<Transfer, DebridError> {
        self.transfers()
            .await?
            .into_iter()
            .find(|transfer| transfer.id == id)
            .ok_or_else(|| DebridError::NotFound(format!("Unable to find transfer with ID {id}")))
    }

    async fn transfers(&self) -> Result<Vec<Transfer>, DebridError> {
        let list: TransferList = self
            .call(self.request(Method::POST, "transfer/list", &[])?)
            .await?;
        Ok(list.transfers.unwrap_or_default())
    }

    fn api_key(&self) -> Result<&str, DebridError> {
        if self.api_key.trim().is_empty() {
            return Err(DebridError::NotConfigured(
                "Premiumize API key not set in the settings".to_owned(),
            ));
        }
        Ok(&self.api_key)
    }

    /// Builds a request for `path` with `params` followed by the API key.
    fn request(
        &self,
        method: Method,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<RequestBuilder, DebridError> {
        let api_key = self.api_key()?;
        Ok(self
            .http
            .client()
            .request(method, format!("{}{path}", self.base_url))
            .query(params)
            .query(&[("apikey", api_key)])
            .timeout(self.timeout))
    }

    async fn call<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, DebridError> {
        let response = self.http.send(request).await?;
        let status = response.status();
        let body = response.text().await?;
        parse_response(status, &body)
    }
}

/// Fields every Premiumize response may carry.
#[derive(Debug, Deserialize)]
struct Envelope {
    status: Option<String>,
    message: Option<String>,
    code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Account {
    customer_id: IdValue,
    /// A unix time, or `false`/`null` for accounts without premium.
    premium_until: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TransferList {
    transfers: Option<Vec<Transfer>>,
}

/// A transfer (torrent) of the account.
#[derive(Debug, Deserialize)]
struct Transfer {
    id: String,
    name: Option<String>,
    message: Option<String>,
    status: Option<String>,
    /// Completion as a fraction between 0 and 1.
    progress: Option<f64>,
    folder_id: Option<String>,
    /// The single file of a transfer that produced no folder.
    file_id: Option<String>,
    /// The magnet link or URL the transfer was created from.
    src: Option<String>,
}

impl Transfer {
    /// Converts to the common form; the listing reports neither sizes nor
    /// files, and the folder holding the result is the only link.
    fn to_remote(&self) -> RemoteTorrent {
        let name = self.name.clone().unwrap_or_default();
        RemoteTorrent {
            id: self.id.clone(),
            original_filename: Some(name.clone()),
            filename: name,
            hash: self.src.clone().unwrap_or_default(),
            progress: percent(self.progress.unwrap_or(1.0)),
            status: self.status.clone(),
            message: self.message.clone(),
            files: Some(Vec::new()),
            links: Some(self.folder_id.iter().cloned().collect()),
            speed: Some(0),
            seeders: Some(0),
            ..RemoteTorrent::default()
        }
    }
}

#[derive(Debug, Deserialize)]
struct Created {
    id: Option<String>,
}

fn created_id(created: Created, what: &str) -> Result<String, DebridError> {
    created
        .id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| DebridError::Provider(format!("Unable to add {what}")))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction of 1 times 100 always fits; NaN and overflow saturate"
)]
fn percent(fraction: f64) -> i64 {
    (fraction * 100.0) as i64
}

/// An identifier Premiumize sends either as a number or as a string.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IdValue {
    Number(i64),
    Text(String),
}

impl fmt::Display for IdValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(number) => write!(f, "{number}"),
            Self::Text(text) => f.write_str(text),
        }
    }
}

fn parse_response<T: DeserializeOwned>(status: StatusCode, body: &str) -> Result<T, DebridError> {
    if status == StatusCode::NO_CONTENT || body.trim().is_empty() {
        return Err(DebridError::InvalidResponse(
            "Premiumize returned no result".to_owned(),
        ));
    }
    let envelope: Envelope = serde_json::from_str(body).map_err(|error| {
        if status.is_success() {
            unreadable(body, &error)
        } else {
            DebridError::InvalidResponse(format!(
                "Premiumize returned HTTP {status}: {}",
                excerpt(body)
            ))
        }
    })?;
    if envelope.status.as_deref() != Some("success")
        && let Some(message) = envelope.message
    {
        return Err(provider_error(message, envelope.code.as_deref()));
    }
    serde_json::from_str(body).map_err(|error| unreadable(body, &error))
}

fn provider_error(message: String, code: Option<&str>) -> DebridError {
    if is_rate_limit(&message) || code.is_some_and(is_rate_limit) {
        DebridError::RateLimited {
            message,
            retry_after: DEFAULT_RATE_LIMIT_COOLDOWN,
        }
    } else {
        DebridError::Provider(message)
    }
}

fn is_rate_limit(text: &str) -> bool {
    let text = text.to_lowercase();
    RATE_LIMIT_MARKERS
        .iter()
        .any(|marker| text.contains(marker))
}

fn unreadable(body: &str, error: &serde_json::Error) -> DebridError {
    DebridError::InvalidResponse(format!(
        "Unable to read the Premiumize response ({error}). Response was: {}",
        excerpt(body)
    ))
}

fn excerpt(body: &str) -> &str {
    body.char_indices()
        .nth(BODY_EXCERPT_CHARS)
        .map_or(body, |(end, _)| &body[..end])
}

/// Writes `remote` into the torrent's `rd_*` fields.
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
    torrent.client_kind = Some(Provider::Premiumize);
    torrent.rd_host.clone_from(&remote.host);
    torrent.rd_split = Some(remote.split);
    torrent.rd_progress = Some(remote.progress);
    torrent.rd_added = remote.added;
    torrent.rd_ended = remote.ended;
    torrent.rd_speed = remote.speed;
    torrent.rd_seeders = remote.seeders;
    torrent.rd_status_raw.clone_from(&remote.status);
    torrent.rd_status = Some(status_of(remote.status.as_deref()));
}

/// Maps a Premiumize transfer status; anything unknown (`error`, `timeout`,
/// `banned`, `deleted`, ...) is an error.
fn status_of(status: Option<&str>) -> TorrentStatus {
    match status {
        Some("waiting" | "queued") => TorrentStatus::Processing,
        Some("running") => TorrentStatus::Downloading,
        Some("seeding" | "finished") => TorrentStatus::Finished,
        _ => TorrentStatus::Error,
    }
}

/// The download infos found below a folder.
type FolderInfos<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<DownloadInfo>, DebridError>> + Send + 'a>>;

#[derive(Debug, Deserialize)]
struct Folder {
    content: Option<Vec<Item>>,
}

/// A file or folder in the cloud storage.
#[derive(Debug, Deserialize)]
struct Item {
    id: Option<String>,
    name: Option<String>,
    size: Option<i64>,
    #[serde(rename = "type")]
    kind: Option<String>,
    link: Option<String>,
}

fn non_blank(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.trim().is_empty())
}

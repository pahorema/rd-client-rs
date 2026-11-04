//! Client for the Debrid-Link seedbox API (`https://debrid-link.fr/api/v2/`).

use std::fmt;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use rdc_model::{DownloadInfo, Provider, RemoteFile, Torrent, TorrentStatus};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::{DeserializeOwned, IgnoredAny};

use crate::error::DebridError;
use crate::http::{DEFAULT_RATE_LIMIT_COOLDOWN, ProviderHttp};
use crate::provider::{DebridUser, ProviderConfig, RemoteTorrent};

/// `rd_status_raw` of a torrent the provider no longer knows.
const DELETED: &str = "deleted";

/// Base URL of the public Debrid-Link API.
pub const DEFAULT_BASE_URL: &str = "https://debrid-link.fr/api/v2/";

/// Torrents requested per listing page (the API maximum).
const PAGE_SIZE: &str = "50";

/// Client for a Debrid-Link account.
///
/// Every response is wrapped in a `{success, value, error}` envelope; a
/// failed envelope becomes a [`DebridError::Provider`] carrying the API error code.
#[derive(Clone)]
pub struct DebridLink {
    http: ProviderHttp,
    api_key: String,
    timeout: Duration,
    base_url: String,
}

impl fmt::Debug for DebridLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DebridLink")
            .field("api_key", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl DebridLink {
    /// Creates a client for the account of `config.api_key`; every request
    /// times out after `config.timeout`.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self {
            http,
            api_key: config.api_key.clone(),
            timeout: config.timeout,
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// Sends requests to `base_url` instead of the public API.
    #[must_use]
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = if base_url.ends_with('/') {
            base_url.to_owned()
        } else {
            format!("{base_url}/")
        };
        self
    }

    /// Returns the account; the expiration is derived from the remaining
    /// premium seconds.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let infos: UserInfos = self
            .send(self.request(Method::GET, "account/infos")?)
            .await?
            .unwrap_or_default();
        let expiration = (infos.premium_left > 0)
            .then(|| {
                Timestamp::now()
                    .checked_add(SignedDuration::from_secs(infos.premium_left))
                    .ok()
            })
            .flatten();
        Ok(DebridUser {
            username: infos.username,
            expiration,
            premium: false,
        })
    }

    /// Lists every torrent of the seedbox, page by page until an empty page.
    pub async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        let mut torrents = Vec::new();
        let mut page: u32 = 0;
        loop {
            let page_text = page.to_string();
            let request = self
                .request(Method::GET, "seedbox/list")?
                .query(&[("page", page_text.as_str()), ("perPage", PAGE_SIZE)]);
            let items: Vec<SeedboxTorrent> = self.send(request).await?.unwrap_or_default();
            if items.is_empty() {
                return Ok(torrents);
            }
            torrents.extend(items.into_iter().map(SeedboxTorrent::into_remote));
            page += 1;
        }
    }

    /// Adds a magnet link and returns the id of the new torrent (empty when
    /// the API returns none).
    pub async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError> {
        let request = self.request(Method::POST, "seedbox/add")?.form(&[
            ("url", magnet),
            ("wait", "false"),
            ("async", "false"),
        ]);
        self.add(request).await
    }

    /// Uploads a `.torrent` file and returns the id of the new torrent (empty
    /// when the API returns none).
    pub async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError> {
        let file = Part::bytes(bytes.to_vec())
            .file_name("1.torrent")
            .mime_str("application/x-bittorrent")?;
        let request = self
            .request(Method::POST, "seedbox/add")?
            .multipart(Form::new().part("file", file));
        self.add(request).await
    }

    async fn add(&self, request: RequestBuilder) -> Result<String, DebridError> {
        let torrent: Option<SeedboxTorrent> =
            self.send(request).await.map_err(rate_limit_from_text)?;
        Ok(torrent.and_then(|torrent| torrent.id).unwrap_or_default())
    }

    /// Removes the torrent from the seedbox; a torrent without provider id is
    /// left alone.
    pub async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError> {
        let Some(id) = non_blank(torrent.rd_id.as_deref()) else {
            return Ok(());
        };
        let request = self.request(Method::DELETE, &format!("seedbox/{id}/remove"))?;
        self.send::<IgnoredAny>(request).await?;
        Ok(())
    }

    /// Copies the provider state into the torrent's `rd_*` fields.
    ///
    /// Uses `remote` when given and otherwise looks the torrent up; a torrent
    /// the seedbox no longer lists gets `rd_status_raw = "deleted"`.
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
            None => match self.info(id).await {
                Ok(info) => {
                    fetched = info;
                    &fetched
                }
                Err(DebridError::NotFound(_)) => {
                    torrent.rd_status_raw = Some(DELETED.to_owned());
                    return Ok(());
                }
                Err(error) => return Err(error),
            },
        };
        apply(torrent, remote);
        Ok(())
    }

    /// Lists the downloadable files with their direct links; `None` while the
    /// seedbox reports no files yet.
    pub async fn download_infos(
        &self,
        torrent: &Torrent,
    ) -> Result<Option<Vec<DownloadInfo>>, DebridError> {
        let Some(id) = non_blank(torrent.rd_id.as_deref()) else {
            return Ok(None);
        };
        let remote = self.info(id).await?;
        if remote.links.is_none() {
            return Ok(None);
        }
        let filter = torrent.file_filter()?;
        let mut infos = Vec::new();
        for file in remote.files.unwrap_or_default() {
            let Some(link) = file.download_link else {
                continue;
            };
            if filter.is_downloadable(&file.path, file.bytes)? {
                infos.push(DownloadInfo {
                    file_name: Some(file_name_of(&file.path).to_owned()),
                    restricted_link: link,
                });
            }
        }
        Ok(Some(infos))
    }

    async fn info(&self, id: &str) -> Result<RemoteTorrent, DebridError> {
        let request = self
            .request(Method::GET, "seedbox/list")?
            .query(&[("ids", id)]);
        let items: Vec<SeedboxTorrent> = self.send(request).await?.unwrap_or_default();
        items
            .into_iter()
            .next()
            .map(SeedboxTorrent::into_remote)
            .ok_or_else(|| DebridError::NotFound(format!("Debrid-Link torrent {id} was not found")))
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, DebridError> {
        if self.api_key.trim().is_empty() {
            return Err(DebridError::NotConfigured(
                "The Debrid-Link API key is not set in the settings".to_owned(),
            ));
        }
        Ok(self
            .http
            .client()
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(&self.api_key)
            .timeout(self.timeout))
    }

    async fn send<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> Result<Option<T>, DebridError> {
        let response = self.http.send(request).await.inspect_err(|error| {
            if let DebridError::Transport(source) = error
                && source.is_timeout()
            {
                tracing::error!("the connection to Debrid-Link timed out: {source}");
            }
        })?;
        let status = response.status();
        if status == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let body = response.text().await?;
        parse_envelope(status, &body)
    }
}

/// The wrapper around every API response.
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    value: Option<T>,
    error: Option<String>,
}

fn parse_envelope<T: DeserializeOwned>(
    status: StatusCode,
    body: &str,
) -> Result<Option<T>, DebridError> {
    let envelope: Envelope<T> = match serde_json::from_str(body) {
        Ok(envelope) => envelope,
        Err(error) if status.is_success() => {
            return Err(DebridError::InvalidResponse(format!(
                "Unable to read the Debrid-Link response: {error}"
            )));
        }
        // Typically an HTML error page of a proxy or a maintenance page.
        Err(_) => return Err(http_error(status)),
    };
    if !envelope.success {
        if let Some(error) = envelope.error.filter(|error| !error.trim().is_empty()) {
            return Err(DebridError::Provider(format!("Debrid-Link error: {error}")));
        }
        if !status.is_success() {
            return Err(http_error(status));
        }
        return Err(DebridError::InvalidResponse(
            "Debrid-Link reported a failure without an error code".to_owned(),
        ));
    }
    if !status.is_success() {
        return Err(http_error(status));
    }
    Ok(envelope.value)
}

fn http_error(status: StatusCode) -> DebridError {
    DebridError::Provider(format!(
        "Debrid-Link answered with unexpected HTTP status {status}"
    ))
}

/// Treats an add failure whose text asks to slow down as a two-minute rate limit.
fn rate_limit_from_text(error: DebridError) -> DebridError {
    if matches!(error, DebridError::RateLimited { .. }) {
        return error;
    }
    let message = error.to_string();
    let lowercase = message.to_lowercase();
    if lowercase.contains("slow_down") || lowercase.contains("rate limit exceeded") {
        DebridError::RateLimited {
            message,
            retry_after: DEFAULT_RATE_LIMIT_COOLDOWN,
        }
    } else {
        error
    }
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

/// Last component of a provider file path.
fn file_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn apply(torrent: &mut Torrent, remote: &RemoteTorrent) {
    if !remote.filename.trim().is_empty() {
        torrent.rd_name = Some(remote.filename.clone());
    }
    if let Some(original) = non_blank(remote.original_filename.as_deref()) {
        torrent.rd_name = Some(original.to_owned());
    }
    if remote.bytes > 0 {
        torrent.rd_size = Some(remote.bytes);
    } else if remote.original_bytes > 0 {
        torrent.rd_size = Some(remote.original_bytes);
    }
    if let Some(files) = &remote.files {
        torrent.set_files(files);
    }
    torrent.client_kind = Some(Provider::DebridLink);
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

/// Maps the stringified numeric seedbox status.
fn status_of(raw: Option<&str>) -> TorrentStatus {
    match raw {
        Some("1" | "2" | "3") => TorrentStatus::Processing,
        Some("4") => TorrentStatus::Downloading,
        Some("5" | "6" | "100") => TorrentStatus::Finished,
        _ => TorrentStatus::Error,
    }
}

/// `account/infos` value.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct UserInfos {
    username: Option<String>,
    /// Remaining premium time in seconds.
    premium_left: i64,
}

/// A torrent of `seedbox/list`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SeedboxTorrent {
    id: Option<String>,
    name: Option<String>,
    hash_string: Option<String>,
    server_id: Option<String>,
    peers_connected: i64,
    /// 0 stopped, 1 queued for verification, 2 verifying, 3 queued for
    /// download, 4 downloading, 5 queued for seeding, 6 seeding, 100 stored.
    status: i64,
    total_size: i64,
    files: Option<Vec<SeedboxFile>>,
    /// Unix seconds.
    created: i64,
    download_percent: i64,
    /// Bytes per second.
    download_speed: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SeedboxFile {
    name: Option<String>,
    size: i64,
    download_url: Option<String>,
}

impl SeedboxTorrent {
    fn into_remote(self) -> RemoteTorrent {
        let name = self.name.unwrap_or_default();
        let links = self.files.as_ref().map(|files| {
            files
                .iter()
                .filter_map(|file| file.download_url.clone())
                .collect()
        });
        let files = self
            .files
            .unwrap_or_default()
            .into_iter()
            .zip(0_i64..)
            .map(|(file, id)| RemoteFile {
                id,
                path: file.name.unwrap_or_default(),
                bytes: file.size,
                selected: true,
                download_link: file.download_url,
            })
            .collect();
        RemoteTorrent {
            id: self.id.unwrap_or_default(),
            original_filename: Some(name.clone()),
            filename: name,
            hash: self.hash_string.unwrap_or_default(),
            bytes: self.total_size,
            host: Some(self.server_id.unwrap_or_default()),
            progress: self.download_percent,
            status: Some(self.status.to_string()),
            status_code: self.status,
            added: Timestamp::from_second(self.created).ok(),
            files: Some(files),
            links,
            speed: Some(self.download_speed),
            seeders: Some(self.peers_connected),
            ..RemoteTorrent::default()
        }
    }
}

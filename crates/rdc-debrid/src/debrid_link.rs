//! Client for the Debrid-Link seedbox API (`https://debrid-link.fr/api/v2/`).

use std::path::Path;

use async_trait::async_trait;
use jiff::{SignedDuration, Timestamp};
use rdc_model::{Download, DownloadInfo, Provider, RemoteFile, Torrent, TorrentStatus};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::{DeserializeOwned, IgnoredAny};

use crate::endpoint::Endpoint;
use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::provider::{AvailableFile, DebridProvider, DebridUser, ProviderConfig, RemoteTorrent};
use crate::text_rate_limit::classify_add_error;
use crate::values::{base_name, non_blank};

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
#[derive(Debug, Clone)]
pub struct DebridLink {
    http: ProviderHttp,
    endpoint: Endpoint,
}

impl DebridLink {
    /// Creates a client for the account of `config.api_key`.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self {
            http,
            endpoint: Endpoint::new("Debrid-Link", DEFAULT_BASE_URL, config),
        }
    }

    /// Sends requests to `base_url` instead of the public API.
    #[must_use]
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.endpoint.set_base_url(base_url);
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
            self.send(request).await.map_err(classify_add_error)?;
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
                    file_name: Some(base_name(&file.path).to_owned()),
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
        self.endpoint.request(&self.http, method, path)
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

#[async_trait]
impl DebridProvider for DebridLink {
    fn kind(&self) -> Provider {
        Provider::DebridLink
    }

    async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        DebridLink::list_torrents(self).await
    }

    async fn user(&self) -> Result<DebridUser, DebridError> {
        DebridLink::user(self).await
    }

    async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError> {
        DebridLink::add_magnet(self, magnet).await
    }

    async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError> {
        DebridLink::add_torrent_file(self, bytes).await
    }

    /// Debrid-Link has no cache lookup by hash; always empty.
    async fn available_files(&self, _hash: &str) -> Result<Vec<AvailableFile>, DebridError> {
        Ok(Vec::new())
    }

    /// Every file of a seedbox torrent is downloaded, so nothing is sent.
    async fn select_files(&self, torrent: &Torrent) -> Result<Option<usize>, DebridError> {
        Ok(Some(torrent.files().len()))
    }

    async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError> {
        DebridLink::delete(self, torrent).await
    }

    /// Seedbox links are already direct.
    async fn unrestrict(&self, _torrent: &Torrent, link: &str) -> Result<String, DebridError> {
        Ok(link.to_owned())
    }

    async fn refresh(
        &self,
        torrent: &mut Torrent,
        remote: Option<&RemoteTorrent>,
    ) -> Result<(), DebridError> {
        DebridLink::refresh(self, torrent, remote).await
    }

    async fn download_infos(
        &self,
        torrent: &Torrent,
    ) -> Result<Option<Vec<DownloadInfo>>, DebridError> {
        DebridLink::download_infos(self, torrent).await
    }

    async fn file_name(&self, download: &Download) -> Result<String, DebridError> {
        download_file_name(download)
    }

    /// Single-file torrents sit at the mount root, the others in a folder
    /// named after the torrent.
    fn symlink_path(
        &self,
        torrent: &Torrent,
        download: &Download,
    ) -> Result<Option<String>, DebridError> {
        let (Some(name), Some(file_name)) = (&torrent.rd_name, &download.file_name) else {
            return Ok(None);
        };
        if torrent.files().len() == 1 {
            return Ok(Some(file_name.clone()));
        }
        Ok(Some(
            Path::new(name)
                .join(file_name)
                .to_string_lossy()
                .into_owned(),
        ))
    }
}

/// The stored file name of a download, else the name derived from its link.
fn download_file_name(download: &Download) -> Result<String, DebridError> {
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

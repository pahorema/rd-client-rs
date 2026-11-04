//! Client of the Real-Debrid REST API.

use jiff::{SignedDuration, Timestamp};
use rdc_model::{Provider, RemoteFile, Torrent, TorrentDownloadAction, TorrentStatus};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::OnceCell;

use crate::endpoint::Endpoint;
use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::json;
use crate::provider::{DebridUser, ProviderConfig, RemoteTorrent};
use crate::text_rate_limit::classify_add_error;

/// API host used when no alternative hostname is configured.
pub const DEFAULT_API_HOSTNAME: &str = "api.real-debrid.com";

/// Error code Real-Debrid uses for an unknown torrent or link.
const RESOURCE_NOT_FOUND: i64 = 7;

/// Torrents requested per listing page.
const PAGE_SIZE: usize = 5000;

/// Raw status given to torrents Real-Debrid no longer knows.
const DELETED_STATUS: &str = "deleted";

/// Real-Debrid account access.
///
/// Real-Debrid reports timestamps in its server's local time while labelling
/// them UTC; the client reads the server's UTC offset once (from `time/iso`)
/// and corrects the `added` and `ended` times of torrents with it.
#[derive(Debug)]
pub struct RealDebrid {
    endpoint: Endpoint,
    http: ProviderHttp,
    server_offset: OnceCell<SignedDuration>,
}

impl RealDebrid {
    /// Creates a client for `https://<api hostname>/rest/1.0/`; a missing or
    /// blank [`ProviderConfig::api_hostname`] selects [`DEFAULT_API_HOSTNAME`].
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        let hostname = config
            .api_hostname
            .as_deref()
            .map(str::trim)
            .filter(|hostname| !hostname.is_empty())
            .unwrap_or(DEFAULT_API_HOSTNAME);
        Self::with_base_url(config, http, &format!("https://{hostname}/rest/1.0/"))
    }

    /// Creates a client for the REST API rooted at `base_url`.
    pub fn with_base_url(config: &ProviderConfig, http: ProviderHttp, base_url: &str) -> Self {
        Self {
            endpoint: Endpoint::new("Real-Debrid", base_url, config),
            http,
            server_offset: OnceCell::new(),
        }
    }

    /// Returns the account the API key belongs to; the expiration is only
    /// reported while premium time is left.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let user: User = self.call(self.request(Method::GET, "user")?).await?;
        let expiration = if user.premium > 0 {
            user.expiration.as_deref().and_then(parse_timestamp)
        } else {
            None
        };
        Ok(DebridUser {
            username: user.username,
            expiration,
            premium: false,
        })
    }

    /// Lists every torrent of the account, page by page until an empty page.
    ///
    /// The listing carries neither files nor reliable links for unfinished
    /// torrents; refreshing a torrent fetches its details when needed.
    pub async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        let server_offset = self.server_offset().await?;
        let mut torrents = Vec::new();
        let mut offset = 0;
        loop {
            let mut request = self.request(Method::GET, "torrents")?;
            if offset > 0 {
                request = request.query(&[("offset", offset)]);
            }
            let page: Vec<TorrentInfo> = self.call(request.query(&[("limit", PAGE_SIZE)])).await?;
            if page.is_empty() {
                break;
            }
            torrents.extend(page.into_iter().map(|info| info.into_remote(server_offset)));
            offset += PAGE_SIZE;
        }
        Ok(torrents)
    }

    /// Adds a magnet link and returns the id of the new torrent.
    ///
    /// Errors mentioning `slow_down` or `rate limit exceeded` are reported as
    /// [`DebridError::RateLimited`].
    pub async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError> {
        let request = self
            .request(Method::POST, "torrents/addMagnet")?
            .form(&[("magnet", magnet)]);
        self.add(request).await
    }

    /// Uploads a `.torrent` file and returns the id of the new torrent; rate
    /// limits are detected like for [`add_magnet`](Self::add_magnet).
    pub async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError> {
        let request = self
            .request(Method::PUT, "torrents/addTorrent")?
            .body(bytes.to_vec());
        self.add(request).await
    }

    async fn add(&self, request: RequestBuilder) -> Result<String, DebridError> {
        let added: AddResult = self.call(request).await.map_err(classify_add_error)?;
        if added.id.is_empty() {
            return Err(DebridError::InvalidResponse(
                "Real-Debrid did not return the id of the added torrent".to_owned(),
            ));
        }
        tracing::debug!(id = %added.id, "torrent added to Real-Debrid");
        Ok(added.id)
    }

    /// Copies the Real-Debrid state into the torrent's `rd_*` fields.
    ///
    /// Torrents without provider id are left untouched. The torrent details
    /// are fetched when `remote` is missing, unfinished (no end time) or has
    /// no name; a torrent Real-Debrid no longer knows only gets
    /// `rd_status_raw = "deleted"`. The stored file list is only replaced by
    /// a non-empty one, so listings never clear it.
    pub async fn refresh(
        &self,
        torrent: &mut Torrent,
        remote: Option<&RemoteTorrent>,
    ) -> Result<(), DebridError> {
        let Some(id) = torrent.rd_id.clone() else {
            return Ok(());
        };
        let fetched;
        let remote = match remote {
            Some(remote) if remote.ended.is_some() && !remote.filename.is_empty() => remote,
            _ => match self.info(&id).await {
                Ok(info) => {
                    fetched = info;
                    &fetched
                }
                Err(error) if error.is_not_found() => {
                    tracing::debug!(%id, "torrent no longer exists on Real-Debrid");
                    torrent.rd_status_raw = Some(DELETED_STATUS.to_owned());
                    return Ok(());
                }
                Err(error) => return Err(error),
            },
        };
        apply(torrent, remote);
        Ok(())
    }

    /// Selects the files Real-Debrid downloads.
    ///
    /// Manually selected torrents keep the files whose path ends with one of
    /// the manual entries; the torrent's file filter then applies. Returns
    /// `Some(0)` without contacting Real-Debrid when no file is left and
    /// `None` while the torrent has no provider id.
    pub async fn select_files(&self, torrent: &Torrent) -> Result<Option<usize>, DebridError> {
        let Some(id) = torrent.rd_id.as_deref() else {
            return Ok(None);
        };
        let files = torrent.files();
        let manual = torrent.manual_files();
        let manual_only = torrent.download_action == TorrentDownloadAction::DownloadManual;
        let filter = torrent.file_filter()?;
        let mut selected = Vec::new();
        for file in &files {
            if manual_only && !manual.iter().any(|suffix| file.path.ends_with(suffix)) {
                continue;
            }
            if filter.is_downloadable(&file.path, file.bytes)? {
                selected.push(file.id.to_string());
            }
        }
        tracing::debug!(%id, selected = selected.len(), total = files.len(), "selecting Real-Debrid files");
        if selected.is_empty() {
            return Ok(Some(0));
        }
        let request = self
            .request(Method::POST, &format!("torrents/selectFiles/{id}"))?
            .form(&[("files", selected.join(","))]);
        self.execute(request).await?;
        Ok(Some(selected.len()))
    }

    /// Removes the torrent from the account; torrents without provider id
    /// need no request.
    pub async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError> {
        let Some(id) = torrent.rd_id.as_deref() else {
            return Ok(());
        };
        self.execute(self.request(Method::DELETE, &format!("torrents/delete/{id}"))?)
            .await?;
        Ok(())
    }

    /// Details of one torrent, including its files and links.
    async fn info(&self, id: &str) -> Result<RemoteTorrent, DebridError> {
        let server_offset = self.server_offset().await?;
        let request = self.request(Method::GET, &format!("torrents/info/{id}"))?;
        let info: TorrentInfo = self.call(request).await?;
        if info.id.is_empty() {
            return Err(DebridError::NotFound("Resource not found".to_owned()));
        }
        Ok(info.into_remote(server_offset))
    }

    /// The UTC offset of the Real-Debrid server clock, read once.
    async fn server_offset(&self) -> Result<SignedDuration, DebridError> {
        self.server_offset
            .get_or_try_init(|| async {
                let request = self
                    .endpoint
                    .public_request(&self.http, Method::GET, "time/iso")?;
                let text = self.execute(request).await?.unwrap_or_default();
                parse_utc_offset(&text).ok_or_else(|| {
                    DebridError::InvalidResponse(format!(
                        "Unable to read the Real-Debrid server time: {text}"
                    ))
                })
            })
            .await
            .copied()
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, DebridError> {
        self.endpoint.request(&self.http, method, path)
    }

    /// Sends `request`; `None` when the API answered without content.
    async fn execute(&self, request: RequestBuilder) -> Result<Option<String>, DebridError> {
        let response = self.http.send(request).await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(api_error(status, &text));
        }
        Ok((status != StatusCode::NO_CONTENT).then_some(text))
    }

    /// Sends `request` and decodes its JSON answer (the default for no content).
    async fn call<T: DeserializeOwned + Default>(
        &self,
        request: RequestBuilder,
    ) -> Result<T, DebridError> {
        let Some(text) = self.execute(request).await? else {
            return Ok(T::default());
        };
        serde_json::from_str(&text).map_err(|error| {
            DebridError::InvalidResponse(format!(
                "Unable to read the Real-Debrid response ({error}). Response was: {text}"
            ))
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct User {
    username: Option<String>,
    /// Seconds of premium time left.
    #[serde(default)]
    premium: i64,
    expiration: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct AddResult {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Default, Deserialize)]
struct TorrentInfo {
    #[serde(default)]
    id: String,
    filename: Option<String>,
    original_filename: Option<String>,
    hash: Option<String>,
    /// Size of the selected files.
    #[serde(default, deserialize_with = "json::optional_i64")]
    bytes: Option<i64>,
    /// Size of all files.
    #[serde(default, deserialize_with = "json::optional_i64")]
    original_bytes: Option<i64>,
    host: Option<String>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    split: Option<i64>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    progress: Option<i64>,
    status: Option<String>,
    added: Option<String>,
    ended: Option<String>,
    files: Option<Vec<FileInfo>>,
    links: Option<Vec<Option<String>>>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    speed: Option<i64>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    seeders: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct FileInfo {
    #[serde(default)]
    id: i64,
    /// Path inside the torrent, starting with `/`.
    #[serde(default)]
    path: String,
    #[serde(default)]
    bytes: i64,
    #[serde(default, deserialize_with = "json::flag")]
    selected: bool,
}

impl TorrentInfo {
    fn into_remote(self, server_offset: SignedDuration) -> RemoteTorrent {
        let files = self.files.map(|files| {
            files
                .into_iter()
                .map(|file| RemoteFile {
                    id: file.id,
                    path: file.path,
                    bytes: file.bytes,
                    selected: file.selected,
                    download_link: None,
                })
                .collect()
        });
        RemoteTorrent {
            id: self.id,
            filename: self.filename.unwrap_or_default(),
            original_filename: self.original_filename,
            hash: self.hash.unwrap_or_default(),
            bytes: self.bytes.unwrap_or_default(),
            original_bytes: self.original_bytes.unwrap_or_default(),
            host: self.host,
            split: self.split.unwrap_or_default(),
            progress: self.progress.unwrap_or_default(),
            status: self.status,
            message: None,
            status_code: 0,
            added: server_time(self.added.as_deref(), server_offset),
            files,
            links: self
                .links
                .map(|links| links.into_iter().flatten().collect()),
            ended: server_time(self.ended.as_deref(), server_offset),
            speed: self.speed,
            seeders: self.seeders,
        }
    }
}

/// Copies `remote` into the torrent's `rd_*` fields.
fn apply(torrent: &mut Torrent, remote: &RemoteTorrent) {
    if !remote.filename.trim().is_empty() {
        torrent.rd_name = Some(remote.filename.clone());
    }
    if let Some(original) = remote
        .original_filename
        .as_deref()
        .filter(|name| !name.trim().is_empty())
    {
        torrent.rd_name = Some(original.to_owned());
    }
    if remote.bytes > 0 {
        torrent.rd_size = Some(remote.bytes);
    } else if remote.original_bytes > 0 {
        torrent.rd_size = Some(remote.original_bytes);
    }
    if let Some(files) = remote.files.as_deref().filter(|files| !files.is_empty()) {
        torrent.set_files(files);
    }
    torrent.client_kind = Some(Provider::RealDebrid);
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

/// The rd-client status of a raw Real-Debrid status; unknown ones are errors.
fn status_of(status: Option<&str>) -> TorrentStatus {
    match status {
        Some("magnet_conversion") => TorrentStatus::Processing,
        Some("waiting_files_selection") => TorrentStatus::WaitingForFileSelection,
        Some("queued" | "downloading" | "compressing") => TorrentStatus::Downloading,
        Some("downloaded") => TorrentStatus::Finished,
        Some("uploading") => TorrentStatus::Uploading,
        _ => TorrentStatus::Error,
    }
}

/// The real instant of a Real-Debrid timestamp written in server local time.
fn server_time(text: Option<&str>, server_offset: SignedDuration) -> Option<Timestamp> {
    parse_timestamp(text?)?.checked_sub(server_offset).ok()
}

/// Reads the UTC offset of a `time/iso` answer such as `2024-05-01T12:00:00+02:00`.
fn parse_utc_offset(text: &str) -> Option<SignedDuration> {
    let text = text.trim().trim_matches('"');
    if text.ends_with(['Z', 'z']) && text.parse::<Timestamp>().is_ok() {
        return Some(SignedDuration::ZERO);
    }
    let time = jiff::fmt::strtime::parse("%Y-%m-%dT%H:%M:%S%:z", text).ok()?;
    Some(SignedDuration::from_secs(i64::from(
        time.offset()?.seconds(),
    )))
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: Option<String>,
    error_code: Option<i64>,
}

/// Maps an unsuccessful answer: a `{error, error_code}` body becomes the
/// message of its code, anything else is reported verbatim.
fn api_error(status: StatusCode, text: &str) -> DebridError {
    match serde_json::from_str::<ErrorBody>(text) {
        Ok(ErrorBody {
            error: Some(error),
            error_code,
        }) => {
            let message = error_code
                .and_then(error_message)
                .map_or(error, str::to_owned);
            if error_code == Some(RESOURCE_NOT_FOUND) {
                DebridError::NotFound(message)
            } else {
                DebridError::Provider(message)
            }
        }
        _ if text.trim().is_empty() => {
            DebridError::Provider(format!("Real-Debrid answered with HTTP status {status}"))
        }
        _ => DebridError::Provider(text.to_owned()),
    }
}

/// User-facing text of a Real-Debrid error code.
fn error_message(code: i64) -> Option<&'static str> {
    let message = match code {
        -1 => "Internal error",
        1 => "Missing parameter",
        2 => "Bad parameter value",
        3 => "Unknown method",
        4 => "Method not allowed",
        5 => "Slow down",
        6 => "Resource unreachable",
        RESOURCE_NOT_FOUND => "Resource not found",
        8 => "Bad token",
        9 => "Permission denied",
        10 => "Two-Factor authentication needed",
        11 => "Two-Factor authentication pending",
        12 => "Invalid login",
        13 => "Invalid password",
        14 => "Account locked",
        15 => "Account not activated",
        16 => "Unsupported hoster",
        17 => "Hoster in maintenance",
        18 => "Hoster limit reached",
        19 => "Hoster temporarily unavailable",
        20 => "Hoster not available for free users",
        21 => "Too many active downloads",
        22 => "IP Address not allowed",
        23 => "Traffic exhausted",
        24 => "File unavailable",
        25 => "Service unavailable",
        26 => "Upload too big",
        27 => "Upload error",
        28 => "File not allowed",
        29 => "Torrent too big",
        30 => "Torrent file invalid",
        31 => "Action already done",
        32 => "Image resolution error",
        33 => "Torrent already active",
        34 => "Too many requests",
        35 => "Infringing file",
        36 => "Fair Usage Limit",
        _ => return None,
    };
    Some(message)
}

fn parse_timestamp(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::rate_limit::RateLimitCoordinator;

    fn client(api_hostname: Option<&str>) -> RealDebrid {
        let config = ProviderConfig {
            api_key: "key".to_owned(),
            api_hostname: api_hostname.map(str::to_owned),
            timeout: Duration::from_secs(1),
            prefer_zipped_downloads: false,
        };
        let http = ProviderHttp::new(config.timeout, Arc::new(RateLimitCoordinator::new()))
            .expect("client builds");
        RealDebrid::new(&config, http)
    }

    fn user_url(client: &RealDebrid) -> String {
        client.endpoint.url("user").expect("valid url").to_string()
    }

    #[test]
    fn missing_or_blank_hostname_uses_the_default_api_host() {
        for hostname in [None, Some(""), Some("   ")] {
            assert_eq!(
                user_url(&client(hostname)),
                "https://api.real-debrid.com/rest/1.0/user",
                "hostname {hostname:?}"
            );
        }
    }

    #[test]
    fn configured_hostname_replaces_the_api_host() {
        assert_eq!(
            user_url(&client(Some("api-1.real-debrid.com"))),
            "https://api-1.real-debrid.com/rest/1.0/user"
        );
    }

    #[test]
    fn raw_statuses_map_to_torrent_statuses() {
        let expected = [
            ("magnet_error", TorrentStatus::Error),
            ("magnet_conversion", TorrentStatus::Processing),
            (
                "waiting_files_selection",
                TorrentStatus::WaitingForFileSelection,
            ),
            ("queued", TorrentStatus::Downloading),
            ("downloading", TorrentStatus::Downloading),
            ("downloaded", TorrentStatus::Finished),
            ("error", TorrentStatus::Error),
            ("virus", TorrentStatus::Error),
            ("compressing", TorrentStatus::Downloading),
            ("uploading", TorrentStatus::Uploading),
            ("dead", TorrentStatus::Error),
            ("something_new", TorrentStatus::Error),
        ];
        for (raw, status) in expected {
            assert_eq!(status_of(Some(raw)), status, "{raw}");
        }
        assert_eq!(status_of(None), TorrentStatus::Error);
    }
}

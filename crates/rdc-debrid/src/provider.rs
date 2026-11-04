//! The provider abstraction every debrid service implements.

use std::time::Duration;

use async_trait::async_trait;
use jiff::Timestamp;
use rdc_model::{Download, DownloadInfo, Provider, RemoteFile, Torrent};
use serde::Serialize;

use crate::error::DebridError;

/// A torrent as listed by the provider.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteTorrent {
    /// Provider-side identifier (stored as `Torrent::rd_id`).
    pub id: String,
    /// Current name.
    pub filename: String,
    /// Name the torrent was added with, when the provider reports it separately.
    pub original_filename: Option<String>,
    /// Info hash (or whatever the provider uses to identify the content).
    pub hash: String,
    /// Size of the selected files.
    pub bytes: i64,
    /// Size of the whole torrent.
    pub original_bytes: i64,
    /// Provider-specific host or location value.
    pub host: Option<String>,
    /// Provider-specific split size.
    pub split: i64,
    /// Provider progress in percent.
    pub progress: i64,
    /// Raw provider status.
    pub status: Option<String>,
    /// Raw provider status message.
    pub message: Option<String>,
    /// Numeric provider status, when the provider has one.
    pub status_code: i64,
    /// When the torrent was added at the provider.
    pub added: Option<Timestamp>,
    /// Files of the torrent, `None` when the listing does not include them.
    pub files: Option<Vec<RemoteFile>>,
    /// Restricted download links, `None` when the listing does not include them.
    pub links: Option<Vec<String>>,
    /// When the provider finished the torrent.
    pub ended: Option<Timestamp>,
    /// Provider download speed in bytes per second.
    pub speed: Option<i64>,
    /// Connected seeders.
    pub seeders: Option<i64>,
}

/// The account the API key belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebridUser {
    /// Account name.
    pub username: Option<String>,
    /// End of the premium subscription.
    pub expiration: Option<Timestamp>,
    /// Whether the provider explicitly reports a premium account.
    pub premium: bool,
}

/// A file the provider already has cached for an info hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AvailableFile {
    /// File name or path inside the torrent.
    pub filename: String,
    /// Size in bytes.
    pub filesize: i64,
}

/// Settings a provider needs to talk to its API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfig {
    /// API key or token of the account.
    pub api_key: String,
    /// Alternative API hostname (Real-Debrid only), `None` for the default.
    pub api_hostname: Option<String>,
    /// Timeout of every request.
    pub timeout: Duration,
    /// Download whole torrents as one zip when possible (TorBox only).
    pub prefer_zipped_downloads: bool,
}

/// Operations rd-client performs against a debrid service.
///
/// Implementations are long-lived (one per configured provider) and may keep
/// state between calls, such as incremental listing cursors.
#[async_trait]
pub trait DebridProvider: Send + Sync + std::fmt::Debug {
    /// Which provider this is.
    fn kind(&self) -> Provider;

    /// Lists every torrent in the account.
    async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError>;

    /// Returns the account the API key belongs to.
    async fn user(&self) -> Result<DebridUser, DebridError>;

    /// Adds a magnet link and returns the provider id of the new torrent.
    async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError>;

    /// Adds a `.torrent` file and returns the provider id of the new torrent.
    async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError>;

    /// Lists files the provider already has for `hash` (lowercase hex info hash).
    async fn available_files(&self, hash: &str) -> Result<Vec<AvailableFile>, DebridError>;

    /// Tells the provider which files to download.
    ///
    /// Returns `None` when the file list is not available yet (try again later),
    /// `Some(0)` when every file was filtered out and `Some(n)` otherwise.
    async fn select_files(&self, torrent: &Torrent) -> Result<Option<usize>, DebridError>;

    /// Removes the torrent from the provider.
    async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError>;

    /// Turns a restricted link into a direct download URL.
    async fn unrestrict(&self, torrent: &Torrent, link: &str) -> Result<String, DebridError>;

    /// Copies the provider state into the torrent's `rd_*` fields.
    ///
    /// `remote` is the matching entry of a previous [`list_torrents`](Self::list_torrents)
    /// call; when `None` the provider is queried for this torrent. A torrent
    /// the provider no longer knows gets `rd_status_raw = "deleted"`.
    async fn refresh(
        &self,
        torrent: &mut Torrent,
        remote: Option<&RemoteTorrent>,
    ) -> Result<(), DebridError>;

    /// Lists the files to download to the host.
    ///
    /// Returns `None` when the links are not ready yet and an empty list when
    /// every file was filtered out.
    async fn download_infos(
        &self,
        torrent: &Torrent,
    ) -> Result<Option<Vec<DownloadInfo>>, DebridError>;

    /// Name of the file behind a download whose provider entry had no name.
    async fn file_name(&self, download: &Download) -> Result<String, DebridError>;

    /// Path of the file inside the rclone mount, for the symlink downloader.
    ///
    /// Defaults to the torrent-relative download path.
    fn symlink_path(
        &self,
        torrent: &Torrent,
        download: &Download,
    ) -> Result<Option<String>, DebridError> {
        Ok(rdc_model::paths::relative_download_path(torrent, download))
    }
}

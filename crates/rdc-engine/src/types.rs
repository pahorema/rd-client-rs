//! Values crossing the engine facade.

use crate::disk_space::DiskSpaceStatus;
use jiff::Timestamp;
use rdc_model::{
    DownloadClient, Torrent, TorrentDownloadAction, TorrentFinishedAction,
    TorrentHostDownloadAction, settings::TorrentDefaults,
};
use serde::Serialize;

/// Per-torrent configuration copied when adding a torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentTemplate {
    /// Category and download subdirectory.
    pub category: Option<String>,
    /// Provider file-selection policy.
    pub download_action: TorrentDownloadAction,
    /// Cleanup policy after successful completion.
    pub finished_action: TorrentFinishedAction,
    /// Minutes before cleanup.
    pub finished_action_delay: i32,
    /// Whether to download provider files locally.
    pub host_download_action: TorrentHostDownloadAction,
    /// Minimum file size in MiB.
    pub download_min_size: i32,
    /// Include expression, taking precedence over exclusion.
    pub include_regex: Option<String>,
    /// Exclude expression.
    pub exclude_regex: Option<String>,
    /// Comma-separated selected path suffixes.
    pub download_manual_files: Option<String>,
    /// Host downloader.
    pub download_client: DownloadClient,
    /// Queue priority, lower first.
    pub priority: Option<i32>,
    /// Allowed download retries.
    pub download_retry_attempts: i32,
    /// Allowed whole-torrent retries.
    pub torrent_retry_attempts: i32,
    /// Minutes before deleting errors, zero disables.
    pub delete_on_error: i32,
    /// Maximum provider lifetime in minutes, zero disables.
    pub lifetime: i32,
}

impl Default for TorrentTemplate {
    fn default() -> Self {
        Self::from_defaults(&TorrentDefaults::default(), DownloadClient::Internal)
    }
}

impl TorrentTemplate {
    /// Builds a template from an entry point's settings and the configured downloader.
    pub fn from_defaults(defaults: &TorrentDefaults, download_client: DownloadClient) -> Self {
        Self {
            category: defaults.category.clone(),
            download_action: if defaults.only_download_available_files {
                TorrentDownloadAction::DownloadAvailableFiles
            } else {
                TorrentDownloadAction::DownloadAll
            },
            finished_action: defaults.finished_action,
            finished_action_delay: defaults.finished_action_delay,
            host_download_action: defaults.host_download_action,
            download_min_size: defaults.min_file_size,
            include_regex: defaults.include_regex.clone(),
            exclude_regex: defaults.exclude_regex.clone(),
            download_manual_files: None,
            download_client,
            priority: (defaults.priority > 0).then_some(defaults.priority),
            download_retry_attempts: defaults.download_retry_attempts,
            torrent_retry_attempts: defaults.torrent_retry_attempts,
            delete_on_error: defaults.delete_on_error,
            lifetime: defaults.torrent_lifetime,
        }
    }

    pub(crate) fn into_torrent(self) -> Torrent {
        Torrent {
            category: self.category,
            download_action: self.download_action,
            finished_action: self.finished_action,
            finished_action_delay: self.finished_action_delay,
            host_download_action: self.host_download_action,
            download_min_size: self.download_min_size,
            include_regex: self.include_regex,
            exclude_regex: self.exclude_regex,
            download_manual_files: self.download_manual_files,
            download_client: self.download_client,
            priority: self.priority,
            download_retry_attempts: self.download_retry_attempts,
            torrent_retry_attempts: self.torrent_retry_attempts,
            delete_on_error: self.delete_on_error,
            lifetime: self.lifetime,
            ..Torrent::default()
        }
    }
}

impl From<&Torrent> for TorrentTemplate {
    fn from(t: &Torrent) -> Self {
        Self {
            category: t.category.clone(),
            download_action: t.download_action,
            finished_action: t.finished_action,
            finished_action_delay: t.finished_action_delay,
            host_download_action: t.host_download_action,
            download_min_size: t.download_min_size,
            include_regex: t.include_regex.clone(),
            exclude_regex: t.exclude_regex.clone(),
            download_manual_files: t.download_manual_files.clone(),
            download_client: t.download_client,
            priority: t.priority,
            download_retry_attempts: t.download_retry_attempts,
            torrent_retry_attempts: t.torrent_retry_attempts,
            delete_on_error: t.delete_on_error,
            lifetime: t.lifetime,
        }
    }
}

/// Independently controls removal of database rows, provider content and local files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeleteOptions {
    /// Remove torrent, download and payload rows.
    pub data: bool,
    /// Remove the remote provider torrent.
    pub provider: bool,
    /// Remove the torrent directory beneath the configured download directory.
    pub local_files: bool,
}

/// The eight editable fields of an existing torrent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TorrentUpdate {
    /// Host downloader.
    pub download_client: DownloadClient,
    /// Host transfer policy.
    pub host_download_action: TorrentHostDownloadAction,
    /// Category.
    pub category: Option<String>,
    /// Queue priority.
    pub priority: Option<i32>,
    /// Download retry allowance.
    pub download_retry_attempts: i32,
    /// Torrent retry allowance.
    pub torrent_retry_attempts: i32,
    /// Error retention in minutes.
    pub delete_on_error: i32,
    /// Maximum lifetime in minutes.
    pub lifetime: i32,
}

/// Live transfer counters; extraction uses a total of 100 and percentage done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStats {
    /// Bytes per second.
    pub speed: u64,
    /// Expected bytes, or 100 during extraction.
    pub bytes_total: u64,
    /// Transferred bytes, or extraction percentage.
    pub bytes_done: u64,
}

/// Snapshot of provider cooldown gating new provider adds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitStatus {
    /// Latest cooldown expiration, absent when requests may proceed.
    pub next_dequeue_time: Option<Timestamp>,
    /// Seconds until requests may proceed.
    pub seconds_remaining: f64,
}

/// State changes broadcast to connected clients.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EngineEvent {
    /// Host disk-space pause or resume status.
    DiskSpaceStatus(DiskSpaceStatus),
    /// Provider cooldown started, extended or expired.
    RateLimitStatus(RateLimitStatus),
}

/// Result of a bounded wait for a torrent to leave the provider queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// A provider id has been assigned.
    Dequeued,
    /// The torrent was removed.
    Missing,
    /// The torrent completed or failed without being sent.
    Completed,
    /// The caller's timeout elapsed.
    TimedOut,
    /// Engine shutdown interrupted the wait.
    Shutdown,
}

/// Provider account and application release information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// Provider's canonical name.
    pub provider: String,
    /// Account name, when available.
    pub user_name: Option<String>,
    /// Premium expiration, when available.
    pub expiration: Option<Timestamp>,
    /// Explicit premium flag or an expiration date is present.
    pub premium: bool,
    /// Running version.
    pub current_version: String,
    /// Latest published version, when known.
    pub latest_version: Option<String>,
    /// A published advisory affects the running version.
    pub is_insecure: bool,
    /// Ordinary release notifications are disabled.
    pub disable_update_notification: bool,
}

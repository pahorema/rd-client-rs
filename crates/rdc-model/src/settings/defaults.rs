//! The `*:Default` settings groups: the configuration given to torrents
//! added through one entry point.

use super::field::section;
use crate::{TorrentFinishedAction, TorrentHostDownloadAction};

/// Configuration copied into torrents added through one entry point (the
/// provider auto-import, the qBittorrent API, the web UI or the watch
/// folder); the `<group>:Default:*` keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentDefaults {
    /// Whether provider files are transferred to the host.
    pub host_download_action: TorrentHostDownloadAction,
    /// Category assigned to the torrent.
    pub category: Option<String>,
    /// What to remove once every file reached the host.
    pub finished_action: TorrentFinishedAction,
    /// Minutes to wait before running the finished action.
    pub finished_action_delay: i32,
    /// Select only the files the provider already has available.
    pub only_download_available_files: bool,
    /// Files of at most this many MiB are skipped; 0 disables the rule.
    pub min_file_size: i32,
    /// Only files whose path matches this regular expression are kept.
    pub include_regex: Option<String>,
    /// Files whose path matches this regular expression are skipped.
    pub exclude_regex: Option<String>,
    /// Retries allowed for the whole torrent.
    pub torrent_retry_attempts: i32,
    /// Retries allowed per download.
    pub download_retry_attempts: i32,
    /// Minutes in error before the torrent is deleted; 0 disables.
    pub delete_on_error: i32,
    /// Maximum lifetime in minutes; 0 disables.
    pub torrent_lifetime: i32,
    /// Queue priority, 1 is the highest; 0 means no priority.
    pub priority: i32,
}

impl Default for TorrentDefaults {
    fn default() -> Self {
        Self {
            host_download_action: TorrentHostDownloadAction::DownloadAll,
            category: None,
            finished_action: TorrentFinishedAction::RemoveAllTorrents,
            finished_action_delay: 0,
            only_download_available_files: true,
            min_file_size: 0,
            include_regex: None,
            exclude_regex: None,
            torrent_retry_attempts: 1,
            download_retry_attempts: 3,
            delete_on_error: 0,
            torrent_lifetime: 0,
            priority: 0,
        }
    }
}

section! {
    TorrentDefaults {
        "HostDownloadAction" => host_download_action: TorrentHostDownloadAction {
            display_name: "Post Torrent Download Action",
            description: "When a torrent is finished downloading on your debrid provider, perform this action. \
                Use this setting if you only want to add files to your debrid provider \
                but not download them to the host.",
        },
        "Category" => category: Option<String> {
            display_name: "Category",
            description: "When a torrent is imported assign it this category.",
        },
        "FinishedAction" => finished_action: TorrentFinishedAction {
            display_name: "Post Download Action",
            description: "When all files are downloaded from the provider to the host, perform this action. \
                Does not apply when using the symlink downloader.",
        },
        "FinishedActionDelay" => finished_action_delay: i32 {
            display_name: "Finished Action Delay",
            description: "When all files are downloaded from the provider to the host, \
                wait this many minutes before performing the action above.",
        },
        "OnlyDownloadAvailableFiles" => only_download_available_files: bool {
            display_name: "Only download available files on debrid provider",
            description: "When selected, it will only download files in the torrent \
                that have already been downloaded by the debrid provider. \
                You can use this in combination with the Minimum file size setting below.",
        },
        "MinFileSize" => min_file_size: i32 {
            display_name: "Minimum file size to download",
            description: "Files that are smaller than this setting (in MB) are skipped and not downloaded. \
                When set to 0 all files are downloaded. \
                When downloading from Radarr or Sonarr it's recommended to keep this setting at at least a few MB \
                to avoid the debrid provider having to re-download the torrent.",
        },
        "IncludeRegex" => include_regex: Option<String> {
            display_name: "Include files",
            description: "Select only the files that are matching this regular expression. \
                Only use this setting OR the Exclude files setting, not both.",
        },
        "ExcludeRegex" => exclude_regex: Option<String> {
            display_name: "Exclude files",
            description: "Ignore files that are matching this regular expression. \
                Only use this setting OR the Include files setting, not both.",
        },
        "TorrentRetryAttempts" => torrent_retry_attempts: i32 {
            display_name: "Automatic retry torrent",
            description: "When a single download has failed multiple times (see setting below) \
                or when the torrent itself received an error \
                it will retry the full torrent this many times before marking it failed.",
        },
        "DownloadRetryAttempts" => download_retry_attempts: i32 {
            display_name: "Automatic retry downloads",
            description: "When a single download fails it will retry it this many times \
                before marking it as failed.",
        },
        "DeleteOnError" => delete_on_error: i32 {
            display_name: "Delete download when in error",
            description: "When a download has been in error for this many minutes, \
                delete it from the provider and the client. 0 to disable.",
        },
        "TorrentLifetime" => torrent_lifetime: i32 {
            display_name: "Torrent maximum lifetime",
            description: "The maximum lifetime of a torrent in minutes. \
                When this time has passed, mark the torrent as error. \
                If the torrent is completed and has downloads, the lifetime setting will not apply. \
                0 to disable.",
        },
        "Priority" => priority: i32 {
            display_name: "Priority",
            description: "Set the priority of a torrent, 1 = highest, 0 = disabled.",
        },
    }
}

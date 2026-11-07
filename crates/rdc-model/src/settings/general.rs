//! The `General` settings group.

use super::field::section;
use crate::{AuthenticationType, LogLevel};

/// General application behaviour (`General:*` keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneralSettings {
    /// Application log verbosity.
    pub log_level: LogLevel,
    /// Maximum number of torrents downloaded to the host at the same time.
    pub download_limit: i32,
    /// Maximum number of simultaneous unpack processes; 0 disables unpacking.
    pub unpack_limit: i32,
    /// Comma-separated categories exposed through the qBittorrent API.
    pub categories: Option<String>,
    /// Program run when a torrent and all its downloads are finished.
    pub run_on_torrent_complete_file_name: Option<String>,
    /// Arguments of that program, with `%` placeholders.
    pub run_on_torrent_complete_arguments: Option<String>,
    /// How users authenticate.
    pub authentication_type: AuthenticationType,
    /// Directory receiving a copy of every added torrent file or magnet.
    pub copy_added_torrents: Option<String>,
    /// URL of a tracker list appended to added torrents.
    pub tracker_enrichment_list: Option<String>,
    /// Minutes the tracker list is cached; 0 disables caching.
    pub tracker_enrichment_cache_expiration: i32,
    /// Comma-separated tracker keywords that are refused.
    pub banned_trackers: Option<String>,
    /// Hide update notifications (security advisories are still shown).
    pub disable_update_notifications: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            log_level: LogLevel::Error,
            download_limit: 2,
            unpack_limit: 1,
            categories: None,
            run_on_torrent_complete_file_name: None,
            run_on_torrent_complete_arguments: None,
            authentication_type: AuthenticationType::UserNamePassword,
            copy_added_torrents: None,
            tracker_enrichment_list: None,
            tracker_enrichment_cache_expiration: 60,
            banned_trackers: None,
            disable_update_notifications: false,
        }
    }
}

section! {
    GeneralSettings {
        "LogLevel" => log_level: LogLevel {
            display_name: "Log level",
            description: "Recommended level is Warning, set to Debug to get the most info.",
        },
        "DownloadLimit" => download_limit: i32 {
            display_name: "Maximum parallel downloads",
            description: "Maximum amount of torrents that get downloaded to your host at the same time.",
        },
        "UnpackLimit" => unpack_limit: i32 {
            display_name: "Maximum unpack processes",
            description: "Maximum amount of downloads that get unpacked on your host at the same time. \
                Set to 0 to disable unpacking.",
        },
        "Categories" => categories: Option<String> {
            display_name: "Categories",
            description: "Expose these categories through the qBittorrent API. \
                Define multiple categories by separating them with a comma.",
        },
        "RunOnTorrentCompleteFileName" => run_on_torrent_complete_file_name: Option<String> {
            display_name: "Run external program on torrent completion",
            description: "Path to the executable to run when the torrent and all downloads are finished. \
                No arguments should be passed here. \
                When running in Docker, this command will run inside your container!",
        },
        "RunOnTorrentCompleteArguments" => run_on_torrent_complete_arguments: Option<String> {
            display_name: "External program arguments",
            description: "When the executable above is executed, use these parameters. \
                They are split into arguments like a shell command line before the parameters \
                are replaced, so a replaced value always stays within its own argument.\n\
                Supports the following parameters:\n\
                %N: Torrent name\n\
                %L: Category\n\
                %F: Content path (same as root path for multifile torrent)\n\
                %R: Root path (first torrent subdirectory path)\n\
                %D: Save path\n\
                %C: Number of files\n\
                %Z: Torrent size (bytes)\n\
                %I: Info hash",
        },
        "AuthenticationType" => authentication_type: AuthenticationType {
            display_name: "Authentication Type",
            description: "How to authenticate with the client. \
                WARNING: when set to None anyone with access to the URL can use the client without any credentials.",
        },
        "CopyAddedTorrents" => copy_added_torrents: Option<String> {
            display_name: "Copy added torrent files",
            description: "When a torrent file or magnet is added, create a copy in this directory.",
        },
        "TrackerEnrichmentList" => tracker_enrichment_list: Option<String> {
            display_name: "Tracker enrichment list",
            description: "Optional. Specify the URL of a tracker list file to be appended to magnet links and torrent files.",
        },
        "TrackerEnrichmentCacheExpiration" => tracker_enrichment_cache_expiration: i32 {
            display_name: "Tracker enrichment cache expiration",
            description: "The time in minutes to cache the tracker list. Set to 0 to disable caching.",
        },
        "BannedTrackers" => banned_trackers: Option<String> {
            display_name: "Banned Trackers",
            description: "Torrents that come from these trackers will not be allowed, \
                this is a failsafe if you are accidentally downloading from private trackers. \
                Will compare by keyword. Define multiple trackers by separating them with a comma.",
        },
        "DisableUpdateNotifications" => disable_update_notifications: bool {
            display_name: "Disable update notifications",
            description: "Ignore update notifications. \
                You will still be notified if the version you are running has a security vulnerability.",
        },
    }
}

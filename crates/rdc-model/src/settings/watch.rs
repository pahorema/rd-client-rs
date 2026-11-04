//! The `Watch` settings group.

use super::TorrentDefaults;
use super::field::section;

/// The watch folder importing `.torrent` and `.magnet` files (`Watch:*`
/// keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchSettings {
    /// Folder scanned for new files; unset disables the watch folder.
    pub path: Option<String>,
    /// Folder receiving files that failed to import; `<path>/error` when
    /// unset.
    pub error_path: Option<String>,
    /// Folder receiving imported files; `<path>/processed` when unset.
    pub processed_path: Option<String>,
    /// Seconds between scans.
    pub interval: i32,
    /// Configuration of imported torrents (`Watch:Default:*`).
    pub default: TorrentDefaults,
}

impl Default for WatchSettings {
    fn default() -> Self {
        Self {
            path: None,
            error_path: None,
            processed_path: None,
            interval: 60,
            default: TorrentDefaults::default(),
        }
    }
}

section! {
    WatchSettings {
        "Path" => path: Option<String> {
            display_name: "Watch Path",
            description: "Watch this path for .torrent or .magnet files. \
                When a file is found it will be automatically imported.",
        },
        "ErrorPath" => error_path: Option<String> {
            display_name: "Error Path",
            description: "When an error occurs the torrent file is moved to this directory. \
                When unset it will be moved to /error in the watch path.",
        },
        "ProcessedPath" => processed_path: Option<String> {
            display_name: "Processed Path",
            description: "When a torrent file is added successfully it will be moved to this directory. \
                When unset it will be moved to /processed in the watch path.",
        },
        "Interval" => interval: i32 {
            display_name: "Check Interval",
            description: "Time in seconds to check the folder for new files.",
        },
    }
}

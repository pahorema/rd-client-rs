//! The `DownloadClient` settings group.

use super::field::section;
use crate::{DownloadClient, DownloadClientLogLevel};

/// How finished files reach the host (`DownloadClient:*` keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadClientSettings {
    /// Engine used for new torrents.
    pub client: DownloadClient,
    /// Local directory downloads are written to.
    pub download_path: String,
    /// The same directory as seen by the media managers (Sonarr, Radarr).
    pub mapped_path: String,
    /// Speed limit per download in MB/s for the internal downloader; 0 is
    /// unlimited.
    pub max_speed: i32,
    /// Upper bound of the speed slider of the web UI, in MB/s.
    pub max_speed_slider_max: i32,
    /// Parallel connections per download of the internal downloader.
    pub parallel_count: i32,
    /// Number of chunks a download is split into.
    pub chunk_count: i32,
    /// Read/write buffer size of the internal downloader in bytes.
    pub buffer_size: i32,
    /// Connection timeout of the internal downloader in milliseconds.
    pub timeout: i32,
    /// Proxy used by the internal downloader.
    pub proxy_server: Option<String>,
    /// Whether the internal downloader binds to [`bind_ip_address`](Self::bind_ip_address).
    pub bind_to_specific_ip: bool,
    /// Local address the internal downloader binds to.
    pub bind_ip_address: Option<String>,
    /// aria2 JSON-RPC endpoint.
    pub aria2c_url: String,
    /// aria2 RPC secret.
    pub aria2c_secret: String,
    /// Download root on the aria2 host; the download path when unset.
    pub aria2c_download_path: Option<String>,
    /// rclone mount searched by the symlink downloader (a trailing `*`
    /// also searches sub-directories).
    pub rclone_mount_path: String,
    /// Synology Download Station URL.
    pub download_station_url: String,
    /// Synology account name.
    pub download_station_username: Option<String>,
    /// Synology account password.
    pub download_station_password: Option<String>,
    /// Download root on the Synology; its default destination when unset.
    pub download_station_download_path: Option<String>,
    /// Free space in GB below which internal downloads pause; 0 disables.
    pub minimum_free_space_gb: i32,
    /// Minutes between free space checks.
    pub disk_space_check_interval_minutes: i32,
    /// Verbosity of the download client log.
    pub log_level: DownloadClientLogLevel,
}

impl Default for DownloadClientSettings {
    fn default() -> Self {
        Self {
            client: DownloadClient::Internal,
            download_path: "/data/downloads".to_owned(),
            mapped_path: "C:\\Downloads".to_owned(),
            max_speed: 0,
            max_speed_slider_max: 100,
            parallel_count: 8,
            chunk_count: 0,
            buffer_size: 4 * 1024 * 1024,
            timeout: 5000,
            proxy_server: None,
            bind_to_specific_ip: false,
            bind_ip_address: None,
            aria2c_url: "http://127.0.0.1:6800/jsonrpc".to_owned(),
            aria2c_secret: "mysecret123".to_owned(),
            aria2c_download_path: None,
            rclone_mount_path: "/mnt/rd/".to_owned(),
            download_station_url: "http://127.0.0.1:5000".to_owned(),
            download_station_username: None,
            download_station_password: None,
            download_station_download_path: None,
            minimum_free_space_gb: 0,
            disk_space_check_interval_minutes: 5,
            log_level: DownloadClientLogLevel::None,
        }
    }
}

impl DownloadClientSettings {
    /// The save path reported to media managers (Sonarr, Radarr): the
    /// [`mapped_path`](Self::mapped_path) when it is not blank, otherwise
    /// the [`download_path`](Self::download_path), with trailing `\` and
    /// then trailing `/` removed and the platform separator appended.
    pub fn default_save_path(&self) -> String {
        let path = if self.mapped_path.trim().is_empty() {
            &self.download_path
        } else {
            &self.mapped_path
        };
        let trimmed = path.trim_end_matches('\\').trim_end_matches('/');
        format!("{trimmed}{}", std::path::MAIN_SEPARATOR)
    }
}

section! {
    DownloadClientSettings {
        "Client" => client: DownloadClient {
            display_name: "Download client",
            description: "Select which download client to use, see the\n\
                <a href=\"https://github.com/pahorema/rd-client-rs#download-clients\" target=\"_blank\">README</a> \
                for the various options.",
        },
        "DownloadPath" => download_path: String {
            display_name: "Download path",
            description: "Path in the docker container to download files to (i.e. /data/downloads), \
                or a local path when using as a service.",
        },
        "MappedPath" => mapped_path: String {
            display_name: "Mapped path",
            description: "Path where files are downloaded to on your host (i.e. D:\\Downloads). \
                This path is used for *arr to find your downloads.",
        },
        "MaxSpeed" => max_speed: i32 {
            display_name: "Download speed (in MB/s) (only used for the Internal downloader)",
            description: "Maximum download speed in MB/s, applied per concurrent download (not a global limit). \
                The effective total ceiling is this value multiplied by the 'Maximum parallel downloads' setting. \
                For example, 40 with 2 parallel downloads is roughly 80 MB/s total, \
                and 40 with 4 parallel downloads is roughly 160 MB/s total, \
                so overall throughput can appear to exceed the value set here. \
                When set to 0 unlimited speed is used.",
        },
        "MaxSpeedSliderMax" => max_speed_slider_max: i32 {
            display_name: "Download speed slider maximum (in MB/s) (only used for the Internal downloader)",
            description: "Upper bound of the download speed slider shown on the Torrents page \
                when the Internal downloader is selected. Set this to roughly your line speed.",
        },
        "ParallelCount" => parallel_count: i32 {
            display_name: "Parallel connections per download (only used for the Internal downloader)",
            description: "Maximum amount of parallel connections that are used to download a single file to your host. \
                If set to 0 no parallel downloading will be done.",
        },
        "ChunkCount" => chunk_count: i32 {
            display_name: "Chunk Count",
            description: "Split the downloaded file in this amount of chunks.",
        },
        "BufferSize" => buffer_size: i32 {
            display_name: "Buffer Size",
            description: "Buffer size in bytes for the Internal downloader, used to read data and write it to disk.",
        },
        "Timeout" => timeout: i32 {
            display_name: "Connection Timeout (only used for the Internal downloader)",
            description: "Timeout in milliseconds before the downloader times out.",
        },
        "ProxyServer" => proxy_server: Option<String> {
            display_name: "Proxy Server (only used for the Internal downloader)",
            description: "Address of a proxy server to download through (only used for the Internal downloader).",
        },
        "BindToSpecificIp" => bind_to_specific_ip: bool {
            display_name: "Bind to specific IP (Internal downloader only)",
            description: "When enabled, the Internal downloader will bind outbound connections to the IP address below.",
        },
        "BindIpAddress" => bind_ip_address: Option<String> {
            display_name: "Bind IP address (Internal downloader only)",
            description: "Local IP address the Internal downloader binds its downloads to. \
                Must be assigned to an active network adapter on this host.",
        },
        "Aria2cUrl" => aria2c_url: String {
            display_name: "Aria2c URL (only used for the Aria2c Downloader)",
            description: "This is the URL to your Aria2c instance. It must end in /jsonrpc. A common URL is\n\
                http://127.0.0.1:6800/jsonrpc.",
        },
        "Aria2cSecret" => aria2c_secret: String {
            display_name: "Aria2c Secret (only used for the Aria2c Downloader)",
            description: "The secret of your Aria2c instance. Optional.",
        },
        "Aria2cDownloadPath" => aria2c_download_path: Option<String> {
            display_name: "Aria2c Download Path",
            description: "The root path to download the file to on the Aria2c host, \
                if empty use the Download path setting.",
        },
        "RcloneMountPath" => rclone_mount_path: String {
            display_name: "Rclone mount path (only used for the Symlink Downloader)",
            description: "Path where Rclone is mounted. Required for Symlink Downloader. \
                Suffix this path with a * to search subdirectories too.",
        },
        "DownloadStationUrl" => download_station_url: String {
            display_name: "Synology DownloadStation URL",
            description: "The URL to the Synology DownloadStation. A common URL is http://127.0.0.1:5000",
        },
        "DownloadStationUsername" => download_station_username: Option<String> {
            display_name: "Synology DownloadStation Username",
            description: "The username to use when connecting to the Synology DownloadStation. \
                The account needs Download Station and File Station permissions \
                (so the per-download destination folder can be created).",
        },
        "DownloadStationPassword" => download_station_password: Option<String> {
            display_name: "Synology DownloadStation Password",
            description: "The password to use when connecting to the Synology DownloadStation.",
        },
        "DownloadStationDownloadPath" => download_station_download_path: Option<String> {
            display_name: "Synology DownloadStation Download Path",
            description: "The root path to download the file on the Synology DownloadStation host, \
                if empty the default DownloadStation path is used.",
        },
        "MinimumFreeSpaceGB" => minimum_free_space_gb: i32 {
            display_name: "Minimum free disk space (GB) (Internal downloader only)",
            description: "Minimum free disk space in GB. Internal downloader downloads will pause \
                when space falls below this value and will resume only when double this space is available. \
                Set to 0 to disable.",
        },
        "DiskSpaceCheckIntervalMinutes" => disk_space_check_interval_minutes: i32 {
            display_name: "Disk space check interval (minutes)",
            description: "How often to check available disk space, in minutes.",
        },
        "LogLevel" => log_level: DownloadClientLogLevel {
            display_name: "Log level",
            description: "Only set when trying to debug a download client, can generate a lot of logs.",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::MAIN_SEPARATOR;

    use super::*;

    fn save_path(mapped_path: &str, download_path: &str) -> String {
        DownloadClientSettings {
            mapped_path: mapped_path.to_owned(),
            download_path: download_path.to_owned(),
            ..DownloadClientSettings::default()
        }
        .default_save_path()
    }

    #[test]
    fn default_save_path_prefers_the_mapped_path() {
        assert_eq!(
            save_path("C:\\Downloads", "/data/downloads"),
            format!("C:\\Downloads{MAIN_SEPARATOR}")
        );
        assert_eq!(
            DownloadClientSettings::default().default_save_path(),
            format!("C:\\Downloads{MAIN_SEPARATOR}")
        );
    }

    #[test]
    fn default_save_path_falls_back_to_the_download_path() {
        assert_eq!(
            save_path(" ", "/data/downloads"),
            format!("/data/downloads{MAIN_SEPARATOR}")
        );
        assert_eq!(save_path("", "/data/"), format!("/data{MAIN_SEPARATOR}"));
    }

    #[test]
    fn default_save_path_trims_backslashes_then_slashes() {
        assert_eq!(
            save_path("D:\\media\\\\", ""),
            format!("D:\\media{MAIN_SEPARATOR}")
        );
        assert_eq!(
            save_path("/mnt/media\\/", ""),
            format!("/mnt/media\\{MAIN_SEPARATOR}")
        );
        assert_eq!(
            save_path("/mnt/media/\\", ""),
            format!("/mnt/media{MAIN_SEPARATOR}")
        );
    }
}

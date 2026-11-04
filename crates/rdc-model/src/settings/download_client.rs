//! The `DownloadClient` settings group.

use super::field::section;
use crate::DownloadClient;

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
        }
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
    }
}

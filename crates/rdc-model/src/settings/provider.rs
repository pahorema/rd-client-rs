//! The `Provider` settings group.

use super::field::section;
use crate::Provider;

/// The debrid provider account and polling (`Provider:*` keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSettings {
    /// The debrid service in use.
    pub provider: Provider,
    /// API key or token of the account.
    pub api_key: String,
    /// Alternative Real-Debrid API host name.
    pub api_hostname: Option<String>,
    /// Import torrents added directly on the provider.
    pub auto_import: bool,
    /// Delete torrents removed from the provider.
    pub auto_delete: bool,
    /// Request timeout in seconds.
    pub timeout: i32,
    /// Seconds between provider polls.
    pub check_interval: i32,
    /// Maximum torrents downloading on the provider at once; 0 is unlimited.
    pub max_parallel_downloads: i32,
    /// Download whole torbox.app torrents as one zip archive.
    pub prefer_zipped_downloads: bool,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            provider: Provider::RealDebrid,
            api_key: String::new(),
            api_hostname: None,
            auto_import: false,
            auto_delete: false,
            timeout: 10,
            check_interval: 10,
            max_parallel_downloads: 0,
            prefer_zipped_downloads: false,
        }
    }
}

section! {
    ProviderSettings {
        "Provider" => provider: Provider {
            display_name: "Provider",
            description: "The following providers are supported:\n\
                <a href=\"https://real-debrid.com/\" target=\"_blank\" rel=\"noopener\">https://real-debrid.com</a>\n\
                <a href=\"https://alldebrid.com/\" target=\"_blank\" rel=\"noopener\">https://alldebrid.com</a>\n\
                <a href=\"https://www.premiumize.me/\" target=\"_blank\" rel=\"noopener\">https://www.premiumize.me/</a>\n\
                <a href=\"https://debrid-link.com/\" target=\"_blank\" rel=\"noopener\">https://debrid-link.com/</a>\n\
                <a href=\"https://torbox.app/\" target=\"_blank\" rel=\"noopener\">https://torbox.app/</a>\n\
                <a href=\"https://high-way.me/\" target=\"_blank\" rel=\"noopener\">https://high-way.me/</a>\n\
                Only one provider can be used at a time.",
        },
        "ApiKey" => api_key: String {
            display_name: "API Key",
            description: "You can find your API key here:\n\
                <a href=\"https://real-debrid.com/apitoken\" target=\"_blank\" rel=\"noopener\">https://real-debrid.com/apitoken</a>\n\
                or\n\
                <a href=\"https://alldebrid.com/apikeys/\" target=\"_blank\" rel=\"noopener\">https://alldebrid.com/apikeys/</a>\n\
                or\n\
                <a href=\"https://www.premiumize.me/account/\" target=\"_blank\" rel=\"noopener\">https://www.premiumize.me/account/</a>\n\
                or\n\
                <a href=\"https://torbox.app/settings/\" target=\"_blank\" rel=\"noopener\">https://torbox.app/settings/</a>\n\
                or\n\
                <a href=\"https://debrid-link.com/webapp/apikey\" target=\"_blank\" rel=\"noopener\">https://debrid-link.com/webapp/apikey</a>\n\
                For HIGHWAY, log in to your HIGHWAY Center and create a token under \
                \"Sitzungen\" &rarr; \"API-Token erstellen\".",
        },
        "ApiHostname" => api_hostname: Option<String> {
            display_name: "API Hostname (Real-Debrid only)",
            description: "Use this instead of the normal hostname for Real-Debrid API requests. \
                Only used by Real-Debrid. Leave blank to use the default.",
        },
        "AutoImport" => auto_import: bool {
            display_name: "Automatically import and process torrents added to provider",
            description: "When selected, import downloads that were not added through rd-client \
                but have been added directly to your debrid provider.",
        },
        "AutoDelete" => auto_delete: bool {
            display_name: "Automatically delete downloads removed from provider",
            description: "When selected, cancel and delete downloads that have been removed from your debrid provider.",
        },
        "Timeout" => timeout: i32 {
            display_name: "Connection Timeout",
            description: "Timeout in seconds to make a connection to the provider. \
                Increase if you experience timeouts in the logs.",
        },
        "CheckInterval" => check_interval: i32 {
            display_name: "Check Interval",
            description: "The interval in seconds to check the torrents info on the provider's API. \
                Minimum is 3 seconds. When there are no active downloads this interval is multiplied by 3.",
        },
        "MaxParallelDownloads" => max_parallel_downloads: i32 {
            display_name: "Max parallel downloads",
            description: "Limits the number of torrents that will be sent for downloading \
                on the debrid provider at the same time. \
                If set to 0, all downloads will be sent immediately without queuing.",
        },
        "PreferZippedDownloads" => prefer_zipped_downloads: bool {
            display_name: "Prefer zipped downloads",
            description: "TorBox only. When selected, rd-client will try to download the entire torrent \
                as a .zip from TorBox and unpack it instead of downloading each file individually.",
        },
    }
}

//! Download Station server configuration (`SYNO.DownloadStation.Info`).

use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::{Error, Session};

/// Download Station server settings. Speeds are in KB/s, 0 meaning unlimited; a `None`
/// field was not reported and is not sent back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Maximum torrent download speed.
    pub bt_max_download: Option<i64>,
    /// Maximum torrent upload speed.
    pub bt_max_upload: Option<i64>,
    /// Whether the eMule service is enabled.
    pub emule_enabled: Option<bool>,
    /// Maximum eMule download speed.
    pub emul_max_download: Option<i64>,
    /// Maximum eMule upload speed.
    pub emul_max_upload: Option<i64>,
    /// Maximum FTP download speed (shared with HTTP).
    pub ftp_max_download: Option<i64>,
    /// Maximum HTTP download speed (shared with FTP).
    pub http_max_download: Option<i64>,
    /// Maximum download speed of the newsgroup service.
    pub nzb_max_download: Option<i64>,
    /// Whether automatic extraction is enabled for regular users.
    pub unzip_service_enabled: Option<bool>,
    /// Folder new tasks are saved to when they name none, relative to the shared folders.
    pub default_destination: Option<String>,
    /// Default destination of eMule tasks.
    pub emul_default_destination: Option<String>,
}

impl ServerConfig {
    /// The reported settings as `setserverconfig` parameters.
    fn to_params(&self) -> Vec<(&'static str, String)> {
        let numbers = [
            ("bt_max_download", self.bt_max_download),
            ("bt_max_upload", self.bt_max_upload),
            ("emul_max_download", self.emul_max_download),
            ("emul_max_upload", self.emul_max_upload),
            ("ftp_max_download", self.ftp_max_download),
            ("http_max_download", self.http_max_download),
            ("nzb_max_download", self.nzb_max_download),
        ];
        let flags = [
            ("emule_enabled", self.emule_enabled),
            ("unzip_service_enabled", self.unzip_service_enabled),
        ];
        let texts = [
            ("default_destination", &self.default_destination),
            ("emul_default_destination", &self.emul_default_destination),
        ];
        let numbers = numbers
            .into_iter()
            .filter_map(|(name, value)| Some((name, value?.to_string())));
        let flags = flags
            .into_iter()
            .filter_map(|(name, value)| Some((name, value?.to_string())));
        let texts = texts
            .into_iter()
            .filter_map(|(name, value)| Some((name, value.clone()?)));
        numbers.chain(flags).chain(texts).collect()
    }
}

impl Session {
    /// The Download Station server settings of this account.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the round trip.
    pub async fn server_config(&self) -> Result<ServerConfig, Error> {
        self.call(|apis| &apis.download_station_info, "getconfig", &[])
            .await
    }

    /// Stores the Download Station server settings; fields that are `None` stay unchanged.
    ///
    /// To change one setting, read the current settings with [`Session::server_config`],
    /// modify them and send them back.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the round trip.
    pub async fn set_server_config(&self, config: &ServerConfig) -> Result<(), Error> {
        let params = config.to_params();
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let _: IgnoredAny = self
            .call(
                |apis| &apis.download_station_info,
                "setserverconfig",
                &params,
            )
            .await?;
        Ok(())
    }
}

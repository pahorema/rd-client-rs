//! Application settings: the typed tree, the catalogue shown by the web UI
//! and the conversions to and from the `Settings` table rows.
//!
//! Every stored setting is one `Settings(SettingId, Value)` row whose id is
//! its catalogue key (e.g. `General:LogLevel`) and whose value is the text
//! produced by [`SettingValue::stored_text`]. Group nodes are listed by the
//! catalogue but never stored.

mod catalogue;
mod defaults;
mod download_client;
mod field;
mod general;
mod provider;
mod value;

use thiserror::Error;

use self::field::LeafError;

pub use self::catalogue::{SettingDescriptor, catalogue};
pub use self::defaults::TorrentDefaults;
pub use self::download_client::DownloadClientSettings;
pub use self::general::GeneralSettings;
pub use self::provider::ProviderSettings;
pub use self::value::{SettingKind, SettingValue, stored_text_from_json};

/// The typed settings tree; [`Default`] holds the value of every setting
/// before any stored row is applied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    /// `General:*`.
    pub general: GeneralSettings,
    /// `DownloadClient:*`.
    pub download_client: DownloadClientSettings,
    /// `Provider:*`, including `Provider:Default:*`.
    pub provider: ProviderSettings,
}

/// Error returned when a stored row cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApplySettingError {
    /// No stored setting has this key (group nodes included).
    #[error("unknown setting {key}")]
    UnknownKey {
        /// The rejected key.
        key: String,
    },
    /// The stored text does not decode into the setting's type; the
    /// current value was kept.
    #[error("invalid value for setting {key}: {value}")]
    InvalidValue {
        /// The setting key.
        key: String,
        /// The rejected stored text.
        value: String,
    },
}

/// The group a stored key belongs to, with the setting name inside it.
enum Target<'key> {
    General(&'key str),
    DownloadClient(&'key str),
    Provider(&'key str),
    ProviderDefault(&'key str),
}

impl<'key> Target<'key> {
    fn of(key: &'key str) -> Option<Self> {
        let (group, name) = key.split_once(':')?;
        match group {
            "General" => Some(Self::General(name)),
            "DownloadClient" => Some(Self::DownloadClient(name)),
            "Provider" => Some(match name.strip_prefix("Default:") {
                Some(name) => Self::ProviderDefault(name),
                None => Self::Provider(name),
            }),
            _ => None,
        }
    }
}

impl Settings {
    /// The defaults; same as [`Settings::default`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one stored row: `raw` is the `Value` column, `None` for SQL
    /// NULL.
    ///
    /// Booleans accept `True`/`False` in any case; integers accept decimal
    /// (and `#`, `0x` or `&h` hexadecimal) text; enums accept the variant
    /// name (case-sensitive, legacy names included) or its number; strings
    /// are taken verbatim. NULL sets optional strings to `None`, other
    /// strings to empty, numbers to 0, booleans to `false` and enums to the
    /// value 0.
    ///
    /// # Errors
    ///
    /// Returns an error for a key that is not a stored setting, or for text
    /// that does not decode; the setting then keeps its current value.
    pub fn apply(&mut self, key: &str, raw: Option<&str>) -> Result<(), ApplySettingError> {
        let result = match Target::of(key) {
            Some(Target::General(name)) => self.general.apply_leaf(name, raw),
            Some(Target::DownloadClient(name)) => self.download_client.apply_leaf(name, raw),
            Some(Target::Provider(name)) => self.provider.apply_leaf(name, raw),
            Some(Target::ProviderDefault(name)) => self.provider.default.apply_leaf(name, raw),
            None => Err(LeafError::Unknown),
        };
        result.map_err(|error| match error {
            LeafError::Unknown => ApplySettingError::UnknownKey {
                key: key.to_owned(),
            },
            LeafError::Invalid => ApplySettingError::InvalidValue {
                key: key.to_owned(),
                value: raw.unwrap_or_default().to_owned(),
            },
        })
    }

    /// The current value of a stored setting; `None` for unknown keys and
    /// group nodes. Its [`SettingValue::stored_text`] is the text to store.
    pub fn value(&self, key: &str) -> Option<SettingValue> {
        match Target::of(key)? {
            Target::General(name) => self.general.leaf_value(name),
            Target::DownloadClient(name) => self.download_client.leaf_value(name),
            Target::Provider(name) => self.provider.leaf_value(name),
            Target::ProviderDefault(name) => self.provider.default.leaf_value(name),
        }
    }

    /// `(key, stored text)` of every stored setting in catalogue order; with
    /// the defaults these are the rows a new database is seeded with.
    pub fn stored_rows(&self) -> impl Iterator<Item = (&'static str, Option<String>)> + '_ {
        catalogue()
            .iter()
            .filter(|descriptor| descriptor.is_stored())
            .map(|descriptor| {
                let value = self
                    .value(descriptor.key())
                    .expect("every stored catalogue key has a value");
                (descriptor.key(), value.stored_text())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuthenticationType, DownloadClient, DownloadClientLogLevel, LogLevel, Provider,
        TorrentFinishedAction, TorrentHostDownloadAction,
    };

    /// The expected default rows of a `<group>:Default` group.
    fn torrent_default_rows(group: &str) -> Vec<(String, Option<String>)> {
        [
            ("HostDownloadAction", Some("0")),
            ("Category", None),
            ("FinishedAction", Some("1")),
            ("FinishedActionDelay", Some("0")),
            ("OnlyDownloadAvailableFiles", Some("True")),
            ("MinFileSize", Some("0")),
            ("IncludeRegex", None),
            ("ExcludeRegex", None),
            ("TorrentRetryAttempts", Some("1")),
            ("DownloadRetryAttempts", Some("3")),
            ("DeleteOnError", Some("0")),
            ("TorrentLifetime", Some("0")),
            ("Priority", Some("0")),
        ]
        .into_iter()
        .map(|(name, value)| (format!("{group}:Default:{name}"), value.map(str::to_owned)))
        .collect()
    }

    fn owned(rows: Vec<(&str, Option<String>)>) -> Vec<(String, Option<String>)> {
        rows.into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    /// The default stored rows whose key starts with `prefix`.
    fn default_rows(prefix: &str) -> Vec<(&'static str, Option<String>)> {
        Settings::default()
            .stored_rows()
            .filter(|(key, _)| key.starts_with(prefix))
            .collect()
    }

    fn rows(expected: &[(&'static str, Option<&str>)]) -> Vec<(&'static str, Option<String>)> {
        expected
            .iter()
            .map(|&(key, value)| (key, value.map(str::to_owned)))
            .collect()
    }

    #[test]
    fn general_defaults() {
        let expected = rows(&[
            ("General:LogLevel", Some("4")),
            ("General:DownloadLimit", Some("2")),
            ("General:UnpackLimit", Some("1")),
            ("General:Categories", None),
            ("General:RunOnTorrentCompleteFileName", None),
            ("General:RunOnTorrentCompleteArguments", None),
            ("General:AuthenticationType", Some("0")),
            ("General:CopyAddedTorrents", None),
            ("General:TrackerEnrichmentList", None),
            ("General:TrackerEnrichmentCacheExpiration", Some("60")),
            ("General:BannedTrackers", None),
            ("General:DisableUpdateNotifications", Some("False")),
        ]);
        assert_eq!(default_rows("General:"), expected);
    }

    #[test]
    fn download_client_defaults() {
        let expected = rows(&[
            ("DownloadClient:Client", Some("0")),
            ("DownloadClient:DownloadPath", Some("/data/downloads")),
            ("DownloadClient:MappedPath", Some("C:\\Downloads")),
            ("DownloadClient:MaxSpeed", Some("0")),
            ("DownloadClient:MaxSpeedSliderMax", Some("100")),
            ("DownloadClient:ParallelCount", Some("8")),
            ("DownloadClient:ChunkCount", Some("0")),
            ("DownloadClient:BufferSize", Some("4194304")),
            ("DownloadClient:Timeout", Some("5000")),
            ("DownloadClient:ProxyServer", None),
            ("DownloadClient:BindToSpecificIp", Some("False")),
            ("DownloadClient:BindIpAddress", None),
            (
                "DownloadClient:Aria2cUrl",
                Some("http://127.0.0.1:6800/jsonrpc"),
            ),
            ("DownloadClient:Aria2cSecret", Some("mysecret123")),
            ("DownloadClient:Aria2cDownloadPath", None),
            ("DownloadClient:RcloneMountPath", Some("/mnt/rd/")),
            (
                "DownloadClient:DownloadStationUrl",
                Some("http://127.0.0.1:5000"),
            ),
            ("DownloadClient:DownloadStationUsername", None),
            ("DownloadClient:DownloadStationPassword", None),
            ("DownloadClient:DownloadStationDownloadPath", None),
            ("DownloadClient:MinimumFreeSpaceGB", Some("0")),
            ("DownloadClient:DiskSpaceCheckIntervalMinutes", Some("5")),
            ("DownloadClient:LogLevel", Some("5")),
        ]);
        assert_eq!(default_rows("DownloadClient:"), expected);
    }

    #[test]
    fn provider_defaults() {
        let expected = rows(&[
            ("Provider:Provider", Some("0")),
            ("Provider:ApiKey", Some("")),
            ("Provider:ApiHostname", None),
            ("Provider:AutoImport", Some("False")),
            ("Provider:AutoDelete", Some("False")),
            ("Provider:Timeout", Some("10")),
            ("Provider:CheckInterval", Some("10")),
            ("Provider:MaxParallelDownloads", Some("0")),
            ("Provider:PreferZippedDownloads", Some("False")),
        ]);
        let mut all = default_rows("Provider:");
        let defaults = all.split_off(expected.len());
        assert_eq!(all, expected);
        assert_eq!(owned(defaults), torrent_default_rows("Provider"));
    }

    #[test]
    fn provider_groups_apply_stored_rows() {
        let mut settings = Settings::new();
        settings
            .apply("Provider:Provider", Some("HIGHWAY"))
            .unwrap();
        settings.apply("Provider:ApiKey", Some("secret")).unwrap();
        settings
            .apply("Provider:Default:Category", Some("tv"))
            .unwrap();
        settings
            .apply("Provider:Default:FinishedAction", Some("RemoveRealDebrid"))
            .unwrap();
        settings
            .apply("Provider:Default:HostDownloadAction", Some("1"))
            .unwrap();
        settings
            .apply("Provider:Default:OnlyDownloadAvailableFiles", Some("False"))
            .unwrap();
        let provider = &settings.provider;
        assert_eq!(provider.provider, Provider::Highway);
        assert_eq!(provider.api_key, "secret");
        assert_eq!(provider.default.category.as_deref(), Some("tv"));
        assert_eq!(
            provider.default.finished_action,
            TorrentFinishedAction::RemoveProvider
        );
        assert_eq!(
            provider.default.host_download_action,
            TorrentHostDownloadAction::DownloadNone
        );
        assert!(!provider.default.only_download_available_files);
        assert_eq!(
            settings.value("Provider:Default:FinishedAction"),
            Some(SettingValue::Enum(2))
        );
        assert_eq!(settings.value("Provider:Default"), None);
        assert!(matches!(
            settings.apply("Provider:Default", Some("1")),
            Err(ApplySettingError::UnknownKey { .. })
        ));
        assert!(matches!(
            settings.apply("Provider:Default:Provider", Some("1")),
            Err(ApplySettingError::UnknownKey { .. })
        ));
    }

    #[test]
    fn download_client_group_applies_stored_rows() {
        let mut settings = Settings::new();
        settings
            .apply("DownloadClient:Client", Some("Bezzad"))
            .unwrap();
        assert_eq!(settings.download_client.client, DownloadClient::Internal);
        settings
            .apply("DownloadClient:Client", Some("Symlink"))
            .unwrap();
        settings
            .apply("DownloadClient:LogLevel", Some("Debug"))
            .unwrap();
        settings.apply("DownloadClient:DownloadPath", None).unwrap();
        settings
            .apply("DownloadClient:BindToSpecificIp", Some("TRUE"))
            .unwrap();
        let group = &settings.download_client;
        assert_eq!(group.client, DownloadClient::Symlink);
        assert_eq!(group.log_level, DownloadClientLogLevel::Debug);
        assert_eq!(group.download_path, "");
        assert!(group.bind_to_specific_ip);
        assert_eq!(
            settings.value("DownloadClient:DownloadPath"),
            Some(SettingValue::String(Some(String::new())))
        );
        assert_eq!(
            settings.apply("DownloadClient:Client", Some("Aria2")),
            Err(ApplySettingError::InvalidValue {
                key: "DownloadClient:Client".to_owned(),
                value: "Aria2".to_owned(),
            })
        );
        assert_eq!(settings.download_client.client, DownloadClient::Symlink);
    }

    #[test]
    fn applies_each_kind() {
        let mut settings = Settings::new();
        settings.apply("General:LogLevel", Some("Debug")).unwrap();
        settings
            .apply("General:DownloadLimit", Some(" 5 "))
            .unwrap();
        settings
            .apply("General:Categories", Some("tv,movies"))
            .unwrap();
        settings
            .apply("General:AuthenticationType", Some("1"))
            .unwrap();
        settings
            .apply("General:DisableUpdateNotifications", Some("true"))
            .unwrap();
        let general = &settings.general;
        assert_eq!(general.log_level, LogLevel::Debug);
        assert_eq!(general.download_limit, 5);
        assert_eq!(general.categories.as_deref(), Some("tv,movies"));
        assert_eq!(general.authentication_type, AuthenticationType::None);
        assert!(general.disable_update_notifications);
    }

    #[test]
    fn null_resets_to_the_type_default() {
        let mut settings = Settings::new();
        settings.apply("General:LogLevel", None).unwrap();
        settings.apply("General:UnpackLimit", None).unwrap();
        settings.apply("General:Categories", Some("tv")).unwrap();
        settings.apply("General:Categories", None).unwrap();
        assert_eq!(settings.general.log_level, LogLevel::Verbose);
        assert_eq!(settings.general.unpack_limit, 0);
        assert_eq!(settings.general.categories, None);
    }

    #[test]
    fn invalid_text_keeps_the_current_value() {
        let mut settings = Settings::new();
        settings.apply("General:DownloadLimit", Some("7")).unwrap();
        settings.apply("General:LogLevel", Some("Warning")).unwrap();
        let before = settings.clone();
        for (key, raw) in [
            ("General:DownloadLimit", "seven"),
            ("General:LogLevel", "warning"),
            ("General:LogLevel", "42"),
            ("General:DisableUpdateNotifications", "1"),
        ] {
            let error = settings.apply(key, Some(raw)).unwrap_err();
            assert_eq!(
                error,
                ApplySettingError::InvalidValue {
                    key: key.to_owned(),
                    value: raw.to_owned(),
                }
            );
        }
        assert_eq!(settings, before);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let mut settings = Settings::new();
        for key in ["General", "General:Nope", "Nope:LogLevel", "LogLevel", ""] {
            assert_eq!(
                settings.apply(key, Some("1")),
                Err(ApplySettingError::UnknownKey {
                    key: key.to_owned()
                })
            );
            assert_eq!(settings.value(key), None, "key {key:?}");
        }
        assert_eq!(settings, Settings::new());
    }

    #[test]
    fn values_are_typed() {
        let settings = Settings::new();
        assert_eq!(
            settings.value("General:LogLevel"),
            Some(SettingValue::Enum(4))
        );
        assert_eq!(
            settings.value("General:DownloadLimit"),
            Some(SettingValue::Int32(2))
        );
        assert_eq!(
            settings.value("General:Categories"),
            Some(SettingValue::String(None))
        );
        assert_eq!(
            settings.value("General:DisableUpdateNotifications"),
            Some(SettingValue::Boolean(false))
        );
    }

    #[test]
    fn stored_rows_round_trip() {
        let mut settings = Settings::new();
        settings
            .apply("General:LogLevel", Some("Information"))
            .unwrap();
        settings.apply("General:BannedTrackers", Some("")).unwrap();
        settings
            .apply("General:DownloadLimit", Some("0x10"))
            .unwrap();
        let rows: Vec<(&str, Option<String>)> = settings.stored_rows().collect();
        let mut reloaded = Settings::new();
        for (key, text) in &rows {
            reloaded.apply(key, text.as_deref()).unwrap();
        }
        assert_eq!(reloaded, settings);
        assert_eq!(
            settings
                .value("General:DownloadLimit")
                .unwrap()
                .stored_text()
                .as_deref(),
            Some("16")
        );
    }

    #[test]
    fn general_catalogue_entries() {
        let entries: Vec<&SettingDescriptor> = catalogue()
            .iter()
            .filter(|descriptor| descriptor.key().starts_with("General"))
            .collect();
        assert_eq!(entries.len(), 13);
        assert_eq!(entries[0].key(), "General");
        assert_eq!(entries[0].kind(), SettingKind::Object);
        assert_eq!(entries[0].display_name(), Some("General"));
        assert_eq!(entries[0].description(), Some(""));
        let log_level = entries[1];
        assert_eq!(log_level.key(), "General:LogLevel");
        assert_eq!(log_level.kind(), SettingKind::Enum);
        assert_eq!(log_level.display_name(), Some("Log level"));
        assert_eq!(log_level.enum_options(), LogLevel::OPTIONS);
        assert_eq!(entries[2].kind(), SettingKind::Int32);
        assert_eq!(entries[4].kind(), SettingKind::String);
        assert_eq!(entries[12].kind(), SettingKind::Boolean);
        assert_eq!(entries[12].enum_options(), []);
    }
}

//! Application settings: the typed tree, the catalogue shown by the web UI
//! and the conversions to and from the `Settings` table rows.
//!
//! Every stored setting is one `Settings(SettingId, Value)` row whose id is
//! its catalogue key (e.g. `General:LogLevel`) and whose value is the text
//! produced by [`SettingValue::stored_text`]. Group nodes are listed by the
//! catalogue but never stored.

mod catalogue;
mod field;
mod general;
mod value;

use thiserror::Error;

use self::field::LeafError;

pub use self::catalogue::{SettingDescriptor, catalogue};
pub use self::general::GeneralSettings;
pub use self::value::{SettingKind, SettingValue, stored_text_from_json};

/// The typed settings tree; [`Default`] holds the value of every setting
/// before any stored row is applied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    /// `General:*`.
    pub general: GeneralSettings,
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
}

impl<'key> Target<'key> {
    fn of(key: &'key str) -> Option<Self> {
        let (group, name) = key.split_once(':')?;
        match group {
            "General" => Some(Self::General(name)),
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
    use crate::{AuthenticationType, LogLevel};

    #[test]
    fn general_defaults() {
        let rows: Vec<(&str, Option<String>)> = Settings::default()
            .stored_rows()
            .filter(|(key, _)| key.starts_with("General:"))
            .collect();
        let expected = [
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
        ];
        let expected: Vec<(&str, Option<String>)> = expected
            .into_iter()
            .map(|(key, value)| (key, value.map(str::to_owned)))
            .collect();
        assert_eq!(rows, expected);
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

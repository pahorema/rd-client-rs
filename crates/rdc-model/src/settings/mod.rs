//! Application settings: the typed tree, the catalogue shown by the web UI
//! and the conversions to and from the `Settings` table rows.

mod catalogue;
mod field;
mod general;
mod value;

pub use self::catalogue::{SettingDescriptor, catalogue};
pub use self::general::GeneralSettings;
pub use self::value::{SettingKind, SettingValue, stored_text_from_json};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LogLevel;

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

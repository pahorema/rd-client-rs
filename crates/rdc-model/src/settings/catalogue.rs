//! The ordered list of settings shown by the web UI.

use super::value::SettingKind;

/// One entry of the settings catalogue: a group node or a stored setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingDescriptor {
    key: String,
    display_name: Option<&'static str>,
    description: Option<&'static str>,
    kind: SettingKind,
    enum_options: &'static [(i32, &'static str)],
}

impl SettingDescriptor {
    /// The key: group names and the setting name joined by `:`
    /// (e.g. `General:LogLevel`, `Provider:Default:Category`); also the
    /// `Settings.SettingId` of stored settings.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Title shown by the web UI.
    pub fn display_name(&self) -> Option<&'static str> {
        self.display_name
    }

    /// Help text shown by the web UI; may contain HTML links and `\n` line
    /// breaks.
    pub fn description(&self) -> Option<&'static str> {
        self.description
    }

    /// The entry type; [`SettingKind::Object`] for group nodes.
    pub fn kind(&self) -> SettingKind {
        self.kind
    }

    /// `(value, label)` pairs in ascending order for enum settings, empty
    /// otherwise.
    pub fn enum_options(&self) -> &'static [(i32, &'static str)] {
        self.enum_options
    }

    /// Whether this entry is a stored setting rather than a group node.
    pub fn is_stored(&self) -> bool {
        self.kind != SettingKind::Object
    }
}

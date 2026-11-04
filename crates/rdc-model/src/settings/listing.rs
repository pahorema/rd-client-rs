//! The settings listing sent to the web UI.

use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Serialize, Serializer};

use super::{SettingDescriptor, SettingValue, Settings, catalogue};

/// One catalogue entry with its current value.
///
/// Serializes as the web UI expects:
/// `{"key", "value", "displayName", "description", "type", "enumValues"}`,
/// with `enumValues` a map from the value (as a string) to its label for
/// enum settings and `null` otherwise; group nodes have a `null` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingEntry {
    descriptor: &'static SettingDescriptor,
    value: Option<SettingValue>,
}

impl SettingEntry {
    /// The catalogue entry.
    pub fn descriptor(&self) -> &'static SettingDescriptor {
        self.descriptor
    }

    /// The current value; `None` for group nodes.
    pub fn value(&self) -> Option<&SettingValue> {
        self.value.as_ref()
    }
}

impl Serialize for SettingEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let descriptor = self.descriptor;
        let options = descriptor.enum_options();
        let mut entry = serializer.serialize_struct("SettingEntry", 6)?;
        entry.serialize_field("key", descriptor.key())?;
        entry.serialize_field("value", &self.value)?;
        entry.serialize_field("displayName", &descriptor.display_name())?;
        entry.serialize_field("description", &descriptor.description())?;
        entry.serialize_field("type", descriptor.kind().as_str())?;
        entry.serialize_field(
            "enumValues",
            &(!options.is_empty()).then_some(EnumValues(options)),
        )?;
        entry.end()
    }
}

/// Enum options as a JSON object keyed by the value's decimal text.
struct EnumValues(&'static [(i32, &'static str)]);

impl Serialize for EnumValues {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (value, label) in self.0 {
            map.serialize_entry(&value.to_string(), label)?;
        }
        map.end()
    }
}

impl Settings {
    /// Every catalogue entry in order, with the current value of stored
    /// settings.
    pub fn entries(&self) -> impl Iterator<Item = SettingEntry> + '_ {
        catalogue().iter().map(|descriptor| SettingEntry {
            descriptor,
            value: self.value(descriptor.key()),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn lists_every_catalogue_entry_with_its_value() {
        let settings = Settings::new();
        let entries: Vec<SettingEntry> = settings.entries().collect();
        assert_eq!(entries.len(), catalogue().len());
        assert_eq!(entries[0].value(), None);
        assert_eq!(entries[1].value(), Some(&SettingValue::Enum(4)));
    }

    #[test]
    fn serializes_enum_entries_like_the_ui_expects() {
        let settings = Settings::new();
        let entry = settings
            .entries()
            .find(|entry| entry.descriptor().key() == "General:LogLevel")
            .unwrap();
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            json!({
                "key": "General:LogLevel",
                "value": 4,
                "displayName": "Log level",
                "description": "Recommended level is Warning, set to Debug to get the most info.",
                "type": "Enum",
                "enumValues": {"0": "Verbose", "1": "Debug", "2": "Information", "3": "Warning", "4": "Error"}
            })
        );
    }

    #[test]
    fn serializes_group_nodes_and_plain_settings() {
        let settings = Settings::new();
        let json: Vec<serde_json::Value> = settings
            .entries()
            .filter(|entry| {
                matches!(
                    entry.descriptor().key(),
                    "Integrations:Default" | "Watch:Interval" | "Watch:Path"
                )
            })
            .map(|entry| serde_json::to_value(&entry).unwrap())
            .collect();
        assert_eq!(
            json[0],
            json!({
                "key": "Integrations:Default",
                "value": null,
                "displayName": null,
                "description": null,
                "type": "Object",
                "enumValues": null
            })
        );
        assert_eq!(json[1]["value"], json!(null));
        assert_eq!(json[1]["type"], json!("String"));
        assert_eq!(json[2]["value"], json!(60));
        assert_eq!(json[2]["type"], json!("Int32"));
        assert_eq!(json[2]["enumValues"], json!(null));
    }
}

//! Typed setting values and their stored text form.

use serde::{Serialize, Serializer};
use serde_json::Value;

/// The type of a catalogue entry, as exposed to the web UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKind {
    /// A group of settings; has no value and is not stored.
    Object,
    /// One of the values listed in the entry's enum options.
    Enum,
    /// `true` or `false`.
    Boolean,
    /// A 32-bit signed integer.
    Int32,
    /// Free text, possibly absent.
    String,
}

impl SettingKind {
    /// The type name shown to the web UI (`Object`, `Enum`, `Boolean`,
    /// `Int32` or `String`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Object => "Object",
            Self::Enum => "Enum",
            Self::Boolean => "Boolean",
            Self::Int32 => "Int32",
            Self::String => "String",
        }
    }
}

/// The current value of a stored setting.
///
/// Serializes as the plain JSON value the web UI expects: a boolean, a
/// number (enums as their integer), a string or `null`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SettingValue {
    /// A boolean setting.
    Boolean(bool),
    /// An integer setting.
    Int32(i32),
    /// An enum setting, as its stable integer.
    Enum(i32),
    /// A text setting; `None` when unset.
    String(Option<String>),
}

impl SettingValue {
    /// The text stored in the `Settings.Value` column: `True`/`False`,
    /// decimal integers (enums by number), strings verbatim and `None`
    /// (SQL NULL) for an unset string.
    pub fn stored_text(&self) -> Option<String> {
        match self {
            Self::Boolean(true) => Some("True".to_owned()),
            Self::Boolean(false) => Some("False".to_owned()),
            Self::Int32(number) | Self::Enum(number) => Some(number.to_string()),
            Self::String(text) => text.clone(),
        }
    }
}

impl Serialize for SettingValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Boolean(value) => serializer.serialize_bool(*value),
            Self::Int32(number) | Self::Enum(number) => serializer.serialize_i32(*number),
            Self::String(Some(text)) => serializer.serialize_str(text),
            Self::String(None) => serializer.serialize_none(),
        }
    }
}

/// Converts a JSON value sent by the web UI into the text to store.
///
/// Numbers keep their JSON text (`10`, `2.5`), booleans become
/// `True`/`False`, strings are stored as they are and `null` clears the
/// value (SQL NULL). Arrays and objects are stored as their JSON text. The
/// text is not validated here: [`Settings::apply`](super::Settings::apply)
/// decodes it.
pub fn stored_text_from_json(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Bool(true) => Some("True".to_owned()),
        Value::Bool(false) => Some("False".to_owned()),
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.clone()),
        Value::Array(_) | Value::Object(_) => Some(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn stored_text_of_each_kind() {
        assert_eq!(
            SettingValue::Boolean(true).stored_text().as_deref(),
            Some("True")
        );
        assert_eq!(
            SettingValue::Boolean(false).stored_text().as_deref(),
            Some("False")
        );
        assert_eq!(
            SettingValue::Int32(-12).stored_text().as_deref(),
            Some("-12")
        );
        assert_eq!(SettingValue::Enum(4).stored_text().as_deref(), Some("4"));
        assert_eq!(
            SettingValue::String(Some(String::new()))
                .stored_text()
                .as_deref(),
            Some("")
        );
        assert_eq!(SettingValue::String(None).stored_text(), None);
    }

    #[test]
    fn serializes_as_plain_json_values() {
        let values = [
            SettingValue::Boolean(true),
            SettingValue::Int32(60),
            SettingValue::Enum(5),
            SettingValue::String(Some("a".to_owned())),
            SettingValue::String(None),
        ];
        assert_eq!(
            serde_json::to_value(values).unwrap(),
            json!([true, 60, 5, "a", null])
        );
    }

    #[test]
    fn json_from_the_ui_becomes_stored_text() {
        let cases = [
            (json!(10), Some("10")),
            (json!(-3), Some("-3")),
            (json!(2.5), Some("2.5")),
            (json!(true), Some("True")),
            (json!(false), Some("False")),
            (json!("2"), Some("2")),
            (json!(""), Some("")),
            (json!(null), None),
            (json!([1, "a"]), Some("[1,\"a\"]")),
            (json!({"a": 1}), Some("{\"a\":1}")),
        ];
        for (value, expected) in cases {
            assert_eq!(
                stored_text_from_json(&value).as_deref(),
                expected,
                "value {value}"
            );
        }
    }

    #[test]
    fn kind_names_match_the_ui() {
        let names: Vec<&str> = [
            SettingKind::Object,
            SettingKind::Enum,
            SettingKind::Boolean,
            SettingKind::Int32,
            SettingKind::String,
        ]
        .into_iter()
        .map(SettingKind::as_str)
        .collect();
        assert_eq!(names, ["Object", "Enum", "Boolean", "Int32", "String"]);
    }
}

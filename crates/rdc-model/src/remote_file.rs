//! Files of a torrent as reported by the debrid provider.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One file of a torrent as reported by the debrid provider.
///
/// The serde form uses `PascalCase` keys (`Id`, `Path`, `Bytes`, `Selected`,
/// `DownloadLink`), the layout of the `Torrents.RdFiles` column.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteFile {
    /// Provider file id (meaning depends on the provider).
    pub id: i64,
    /// Path inside the torrent, e.g. `folder/sub/file.mkv` (Real-Debrid
    /// prefixes it with `/`).
    pub path: String,
    /// File size in bytes.
    pub bytes: i64,
    /// Whether the file is selected for download on the provider.
    pub selected: bool,
    /// Provider download link, when the provider exposes one per file.
    pub download_link: Option<String>,
}

impl RemoteFile {
    /// Encodes files in the stored column form: a compact JSON array with
    /// `PascalCase` keys and `null` written for absent links.
    pub fn list_to_json(files: &[Self]) -> String {
        serde_json::to_string(files).expect("remote files always serialize to JSON")
    }

    /// Decodes the stored column form.
    ///
    /// Decoding is all-or-nothing and never fails: blank text, invalid JSON,
    /// a JSON `null`, a non-array value or any element with a wrongly typed
    /// field yields an empty list. Keys match case-insensitively (an exact
    /// match wins), unknown keys are ignored, missing fields take their
    /// default and `null` array elements are skipped.
    pub fn list_from_json(text: &str) -> Vec<Self> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        items
            .iter()
            .filter(|item| !item.is_null())
            .map(Self::from_value)
            .collect::<Result<Vec<_>, InvalidField>>()
            .unwrap_or_default()
    }

    fn from_value(value: &Value) -> Result<Self, InvalidField> {
        let object = value.as_object().ok_or(InvalidField)?;
        Ok(Self {
            id: integer(field(object, "Id"))?,
            path: text(field(object, "Path"))?.unwrap_or_default(),
            bytes: integer(field(object, "Bytes"))?,
            selected: boolean(field(object, "Selected"))?,
            download_link: text(field(object, "DownloadLink"))?,
        })
    }
}

/// A stored field whose JSON type does not match.
struct InvalidField;

/// Looks a key up exactly first, then ignoring case.
fn field<'a>(object: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    object.get(name).or_else(|| {
        object
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
    })
}

/// A missing field defaults to 0; anything but an integer is invalid.
fn integer(value: Option<&Value>) -> Result<i64, InvalidField> {
    value.map_or(Ok(0), |value| value.as_i64().ok_or(InvalidField))
}

/// A missing field defaults to `false`; anything but a boolean is invalid.
fn boolean(value: Option<&Value>) -> Result<bool, InvalidField> {
    value.map_or(Ok(false), |value| value.as_bool().ok_or(InvalidField))
}

/// A missing field or `null` is `None`; anything but a string is invalid.
fn text(value: Option<&Value>) -> Result<Option<String>, InvalidField> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(InvalidField),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<RemoteFile> {
        vec![
            RemoteFile {
                id: 1,
                path: "/folder/a.mkv".to_owned(),
                bytes: 1024,
                selected: true,
                download_link: Some("https://example.com/a".to_owned()),
            },
            RemoteFile {
                id: 2,
                path: "folder/b.nfo".to_owned(),
                bytes: 12,
                selected: false,
                download_link: None,
            },
        ]
    }

    #[test]
    fn writes_compact_pascal_case_json_with_nulls() {
        assert_eq!(
            RemoteFile::list_to_json(&sample()),
            r#"[{"Id":1,"Path":"/folder/a.mkv","Bytes":1024,"Selected":true,"DownloadLink":"https://example.com/a"},{"Id":2,"Path":"folder/b.nfo","Bytes":12,"Selected":false,"DownloadLink":null}]"#
        );
        assert_eq!(RemoteFile::list_to_json(&[]), "[]");
    }

    #[test]
    fn round_trips_stored_json() {
        let files = sample();
        assert_eq!(
            RemoteFile::list_from_json(&RemoteFile::list_to_json(&files)),
            files
        );
    }

    #[test]
    fn blank_null_or_invalid_json_is_empty() {
        for text in ["", "   ", "null", "{}", "[", "not json", "[1]", "42"] {
            assert_eq!(
                RemoteFile::list_from_json(text),
                Vec::new(),
                "input {text:?}"
            );
        }
    }

    #[test]
    fn a_wrongly_typed_field_empties_the_whole_list() {
        let text = r#"[{"Id":1,"Path":"a","Bytes":1,"Selected":true},{"Id":"2","Path":"b","Bytes":1,"Selected":true}]"#;
        assert_eq!(RemoteFile::list_from_json(text), Vec::new());
        let text = r#"[{"Id":1,"Path":"a","Bytes":null,"Selected":true}]"#;
        assert_eq!(RemoteFile::list_from_json(text), Vec::new());
        let text = r#"[{"Id":1.5,"Path":"a","Bytes":1,"Selected":true}]"#;
        assert_eq!(RemoteFile::list_from_json(text), Vec::new());
    }

    #[test]
    fn keys_match_case_insensitively_and_unknown_keys_are_ignored() {
        let text =
            r#"[{"id":7,"path":"x.mkv","bytes":9,"selected":true,"downloadLink":"l","extra":[1]}]"#;
        assert_eq!(
            RemoteFile::list_from_json(text),
            vec![RemoteFile {
                id: 7,
                path: "x.mkv".to_owned(),
                bytes: 9,
                selected: true,
                download_link: Some("l".to_owned()),
            }]
        );
    }

    #[test]
    fn exact_key_wins_over_case_insensitive_match() {
        let text = r#"[{"path":"lower","Path":"exact"}]"#;
        assert_eq!(RemoteFile::list_from_json(text)[0].path, "exact");
    }

    #[test]
    fn missing_fields_take_defaults_and_null_elements_are_skipped() {
        let text = r#"[null,{"Path":null}]"#;
        assert_eq!(
            RemoteFile::list_from_json(text),
            vec![RemoteFile::default()]
        );
    }
}

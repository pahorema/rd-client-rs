//! Download status structures returned by the `aria2.tell*` methods.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};

/// State of an aria2 download (`status` key).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum DownloadState {
    /// Currently downloading or seeding.
    Active,
    /// Queued, not started yet.
    #[default]
    Waiting,
    /// Paused.
    Paused,
    /// Stopped because of an error.
    Error,
    /// Stopped and completed.
    Complete,
    /// Removed by the user.
    Removed,
    /// A state this client does not know, kept verbatim.
    Other(String),
}

impl DownloadState {
    /// The wire name of the state.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Paused => "paused",
            Self::Error => "error",
            Self::Complete => "complete",
            Self::Removed => "removed",
            Self::Other(other) => other,
        }
    }
}

impl From<String> for DownloadState {
    fn from(value: String) -> Self {
        match value.as_str() {
            "active" => Self::Active,
            "waiting" => Self::Waiting,
            "paused" => Self::Paused,
            "error" => Self::Error,
            "complete" => Self::Complete,
            "removed" => Self::Removed,
            _ => Self::Other(value),
        }
    }
}

impl fmt::Display for DownloadState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DownloadState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

/// Status of one download, as returned by `aria2.tellStatus` and the list methods.
///
/// aria2 encodes every number as a string; the numeric fields here are already parsed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStatus {
    /// GID of the download.
    pub gid: String,
    /// Current state.
    pub status: DownloadState,
    /// Total length in bytes (0 while unknown).
    #[serde(default, deserialize_with = "number")]
    pub total_length: u64,
    /// Downloaded bytes.
    #[serde(default, deserialize_with = "number")]
    pub completed_length: u64,
    /// Download speed in bytes per second.
    #[serde(default, deserialize_with = "number")]
    pub download_speed: u64,
    /// Code of the last error (aria2 exit status), present on stopped downloads.
    #[serde(default)]
    pub error_code: Option<String>,
    /// Human-readable message of the last error.
    #[serde(default)]
    pub error_message: Option<String>,
    /// Directory the files are saved to.
    #[serde(default)]
    pub dir: String,
    /// Files of the download.
    #[serde(default)]
    pub files: Vec<DownloadFile>,
}

/// One file of a download.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DownloadFile {
    /// Path of the file on the aria2 host.
    #[serde(default)]
    pub path: String,
    /// URIs the file is downloaded from.
    #[serde(default)]
    pub uris: Vec<FileUri>,
}

/// One source URI of a file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct FileUri {
    /// The URI.
    pub uri: String,
    /// `used` when the URI is in use, `waiting` when it is queued.
    #[serde(default)]
    pub status: String,
}

impl DownloadStatus {
    /// Whether any file of this download is fetched from `uri`.
    pub fn has_uri(&self, uri: &str) -> bool {
        self.files
            .iter()
            .flat_map(|file| &file.uris)
            .any(|file_uri| file_uri.uri == uri)
    }
}

/// Deserializes an unsigned integer that aria2 may send as a string or as a number.
fn number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    struct NumberVisitor;

    impl Visitor<'_> for NumberVisitor {
        type Value = u64;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an unsigned integer or a string containing one")
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<u64, E> {
            Ok(value)
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<u64, E> {
            u64::try_from(value).map_err(|_| E::invalid_value(de::Unexpected::Signed(value), &self))
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<u64, E> {
            if value.is_empty() {
                return Ok(0);
            }
            value
                .parse()
                .map_err(|_| E::invalid_value(de::Unexpected::Str(value), &self))
        }

        fn visit_unit<E: de::Error>(self) -> Result<u64, E> {
            Ok(0)
        }
    }

    deserializer.deserialize_any(NumberVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_parsed_from_strings_and_numbers() {
        let status: DownloadStatus = serde_json::from_str(
            r#"{"gid":"a","status":"active","totalLength":"100","completedLength":40,"downloadSpeed":""}"#,
        )
        .unwrap();

        assert_eq!(status.total_length, 100);
        assert_eq!(status.completed_length, 40);
        assert_eq!(status.download_speed, 0);
    }

    #[test]
    fn non_numeric_length_is_rejected() {
        let result = serde_json::from_str::<DownloadStatus>(
            r#"{"gid":"a","status":"active","totalLength":"abc"}"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn unknown_state_is_kept_verbatim() {
        let state: DownloadState = serde_json::from_str(r#""seeding""#).unwrap();

        assert_eq!(state, DownloadState::Other("seeding".to_owned()));
        assert_eq!(state.to_string(), "seeding");
    }

    #[test]
    fn known_states_round_trip_through_their_wire_names() {
        for name in [
            "active", "waiting", "paused", "error", "complete", "removed",
        ] {
            let state = DownloadState::from(name.to_owned());

            assert!(!matches!(state, DownloadState::Other(_)), "{name}");
            assert_eq!(state.as_str(), name);
        }
    }
}

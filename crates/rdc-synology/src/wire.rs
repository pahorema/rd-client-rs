//! Parameter encoding and lenient value decoding shared by the API modules.

use std::fmt;

use serde::Deserializer;
use serde::de::{self, Visitor};
use serde_json::Value;

/// An unsigned integer sent as a number, a numeric string or `null` (0).
pub(crate) fn number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
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

/// Free text that DSM normally sends as a string but occasionally as another JSON value;
/// non-string values are kept as their JSON text, `null` becomes `None`.
pub(crate) fn lenient_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value: Value = serde::Deserialize::deserialize(deserializer)?;
    Ok(match value {
        Value::Null => None,
        Value::String(text) => Some(text),
        other => Some(other.to_string()),
    })
}

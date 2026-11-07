//! Tolerant decoding of provider JSON values whose type varies between
//! answers (numbers sent as text or floats, flags sent as numbers, lists
//! sent as a single object).

use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(untagged)]
enum Number {
    Integer(i64),
    Float(f64),
    Text(String),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Flag {
    Bool(bool),
    Number(Number),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    Many(Vec<T>),
    One(T),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Lossy<T> {
    List(OneOrMany<T>),
    Other(IgnoredAny),
}

/// Decodes an integer sent as a JSON number (floats are truncated) or as
/// numeric text; `null` and unparsable text decode as `None`.
pub(crate) fn optional_i64<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    Ok(Option::<Number>::deserialize(deserializer)?.and_then(Number::into_i64))
}

/// Decodes a flag sent as a JSON boolean or as a number (non-zero is
/// `true`); `null` decodes as `false`.
pub(crate) fn flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(match Option::<Flag>::deserialize(deserializer)? {
        Some(Flag::Bool(value)) => value,
        Some(Flag::Number(number)) => number.into_i64().is_some_and(|value| value != 0),
        None => false,
    })
}

/// Decodes a list that may also be sent as a single object (a list of one);
/// `null` decodes as `None` and any other shape, or an unreadable item, is
/// an error.
pub(crate) fn one_or_many<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<OneOrMany<T>>::deserialize(deserializer)?.map(OneOrMany::into_vec))
}

/// Like [`one_or_many`], but a value of any other shape or with an
/// unreadable item decodes as `None`; only for optional details, such as
/// file trees, that can be fetched again.
pub(crate) fn lossy_one_or_many<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(match Option::<Lossy<T>>::deserialize(deserializer)? {
        Some(Lossy::List(list)) => Some(list.into_vec()),
        Some(Lossy::Other(_)) | None => None,
    })
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::Many(items) => items,
            Self::One(item) => vec![item],
        }
    }
}

impl Number {
    fn into_i64(self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(value),
            Self::Float(value) => float_to_i64(value),
            Self::Text(text) => {
                let text = text.trim();
                text.parse()
                    .ok()
                    .or_else(|| text.parse().ok().and_then(float_to_i64))
            }
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "provider counters fit in i64; the fraction is dropped on purpose"
)]
fn float_to_i64(value: f64) -> Option<i64> {
    value.is_finite().then_some(value as i64)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Sample {
        #[serde(default, deserialize_with = "super::optional_i64")]
        number: Option<i64>,
        #[serde(default, deserialize_with = "super::flag")]
        flag: bool,
    }

    fn sample(json: &str) -> Sample {
        serde_json::from_str(json).expect("valid sample")
    }

    #[test]
    fn numbers_are_read_from_integers_floats_and_text() {
        assert_eq!(sample(r#"{"number": 12}"#).number, Some(12));
        assert_eq!(sample(r#"{"number": 37.9}"#).number, Some(37));
        assert_eq!(sample(r#"{"number": "42"}"#).number, Some(42));
        assert_eq!(sample(r#"{"number": "4.5"}"#).number, Some(4));
    }

    #[test]
    fn missing_null_or_unparsable_numbers_are_none() {
        assert_eq!(sample("{}").number, None);
        assert_eq!(sample(r#"{"number": null}"#).number, None);
        assert_eq!(sample(r#"{"number": "n/a"}"#).number, None);
    }

    #[test]
    fn flags_are_read_from_booleans_and_numbers() {
        assert!(sample(r#"{"flag": true}"#).flag);
        assert!(sample(r#"{"flag": 1}"#).flag);
        assert!(!sample(r#"{"flag": 0}"#).flag);
        assert!(!sample(r#"{"flag": null}"#).flag);
        assert!(!sample("{}").flag);
    }
}

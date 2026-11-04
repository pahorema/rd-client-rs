//! Parsing of stored setting text into typed fields, and the macro that
//! connects a settings group's fields to their catalogue keys.

use super::value::{SettingKind, SettingValue};
use crate::{
    AuthenticationType, DownloadClient, DownloadClientLogLevel, LogLevel, Provider,
    TorrentFinishedAction, TorrentHostDownloadAction,
};

/// A type a stored setting decodes into.
pub(crate) trait Field: Sized {
    /// The catalogue kind of the setting.
    const KIND: SettingKind;
    /// `(value, label)` pairs of an enum setting.
    const OPTIONS: &'static [(i32, &'static str)] = &[];

    /// Decodes the stored text (`None` = SQL NULL); `None` when invalid.
    fn parse(raw: Option<&str>) -> Option<Self>;

    /// The current value.
    fn to_value(&self) -> SettingValue;
}

impl Field for bool {
    const KIND: SettingKind = SettingKind::Boolean;

    /// NULL is `false`; otherwise `True` or `False` in any case, ignoring
    /// surrounding whitespace and NUL characters.
    fn parse(raw: Option<&str>) -> Option<Self> {
        let Some(raw) = raw else {
            return Some(false);
        };
        let text = raw.trim_matches(|c: char| c.is_whitespace() || c == '\0');
        if text.eq_ignore_ascii_case("true") {
            Some(true)
        } else if text.eq_ignore_ascii_case("false") {
            Some(false)
        } else {
            None
        }
    }

    fn to_value(&self) -> SettingValue {
        SettingValue::Boolean(*self)
    }
}

impl Field for i32 {
    const KIND: SettingKind = SettingKind::Int32;

    /// NULL is 0; otherwise a decimal integer with an optional sign, or a
    /// hexadecimal one prefixed with `#`, `0x` or `&h` (two's complement),
    /// ignoring surrounding whitespace.
    fn parse(raw: Option<&str>) -> Option<Self> {
        let Some(raw) = raw else {
            return Some(0);
        };
        let text = raw.trim();
        let hex = ["#", "0x", "0X", "&h", "&H"]
            .into_iter()
            .find_map(|prefix| text.strip_prefix(prefix));
        match hex {
            Some(digits) if digits.starts_with(['+', '-']) => None,
            Some(digits) => u32::from_str_radix(digits, 16).ok().map(u32::cast_signed),
            None => text.parse().ok(),
        }
    }

    fn to_value(&self) -> SettingValue {
        SettingValue::Int32(*self)
    }
}

impl Field for String {
    const KIND: SettingKind = SettingKind::String;

    /// The text as stored; NULL is the empty string.
    fn parse(raw: Option<&str>) -> Option<Self> {
        Some(raw.unwrap_or_default().to_owned())
    }

    fn to_value(&self) -> SettingValue {
        SettingValue::String(Some(self.clone()))
    }
}

impl Field for Option<String> {
    const KIND: SettingKind = SettingKind::String;

    /// The text as stored; NULL is `None`.
    fn parse(raw: Option<&str>) -> Option<Self> {
        Some(raw.map(str::to_owned))
    }

    fn to_value(&self) -> SettingValue {
        SettingValue::String(self.clone())
    }
}

/// Implements [`Field`] for enums: NULL is the value 0, otherwise the
/// variant name (case-sensitive, legacy names included) or its number.
macro_rules! enum_field {
    ($($enum:ty),+ $(,)?) => {
        $(
            impl Field for $enum {
                const KIND: SettingKind = SettingKind::Enum;
                const OPTIONS: &'static [(i32, &'static str)] = <$enum>::OPTIONS;

                fn parse(raw: Option<&str>) -> Option<Self> {
                    raw.unwrap_or("0").parse().ok()
                }

                fn to_value(&self) -> SettingValue {
                    SettingValue::Enum(self.as_i32())
                }
            }
        )+
    };
}

enum_field!(
    AuthenticationType,
    DownloadClient,
    DownloadClientLogLevel,
    LogLevel,
    Provider,
    TorrentFinishedAction,
    TorrentHostDownloadAction,
);

/// Catalogue metadata of one stored setting of a group.
#[derive(Debug)]
pub(crate) struct Leaf {
    /// Key below the group, e.g. `LogLevel` in `General:LogLevel`.
    pub(crate) name: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) kind: SettingKind,
    pub(crate) options: &'static [(i32, &'static str)],
}

/// Why a stored row could not be applied to a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeafError {
    /// The group has no setting with that name.
    Unknown,
    /// The stored text does not decode; the field is unchanged.
    Invalid,
}

/// Connects the fields of a settings group to their catalogue entries.
///
/// Generates, in declaration order, the group's `LEAVES` metadata plus
/// `leaf_value` and `apply_leaf` dispatching on the setting name.
macro_rules! section {
    (
        $group:ty {
            $(
                $name:literal => $field:ident : $ty:ty {
                    display_name: $display_name:expr,
                    description: $description:expr $(,)?
                }
            ),+ $(,)?
        }
    ) => {
        impl $group {
            /// Catalogue metadata of the group's stored settings, in order.
            pub(crate) const LEAVES: &'static [$crate::settings::field::Leaf] = &[$(
                $crate::settings::field::Leaf {
                    name: $name,
                    display_name: $display_name,
                    description: $description,
                    kind: <$ty as $crate::settings::field::Field>::KIND,
                    options: <$ty as $crate::settings::field::Field>::OPTIONS,
                },
            )+];

            /// The current value of the setting `name` of this group.
            pub(crate) fn leaf_value(
                &self,
                name: &str,
            ) -> Option<$crate::settings::SettingValue> {
                match name {
                    $($name => Some($crate::settings::field::Field::to_value(&self.$field)),)+
                    _ => None,
                }
            }

            /// Applies the stored text of the setting `name` of this group.
            pub(crate) fn apply_leaf(
                &mut self,
                name: &str,
                raw: Option<&str>,
            ) -> Result<(), $crate::settings::field::LeafError> {
                match name {
                    $($name => {
                        self.$field = <$ty as $crate::settings::field::Field>::parse(raw)
                            .ok_or($crate::settings::field::LeafError::Invalid)?;
                        Ok(())
                    })+
                    _ => Err($crate::settings::field::LeafError::Unknown),
                }
            }
        }
    };
}

pub(crate) use section;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booleans_parse_true_and_false_in_any_case() {
        let cases = [
            (Some("True"), Some(true)),
            (Some("false"), Some(false)),
            (Some(" TRUE \n"), Some(true)),
            (Some("\0False\0"), Some(false)),
            (None, Some(false)),
            (Some("1"), None),
            (Some("0"), None),
            (Some("yes"), None),
            (Some(""), None),
        ];
        for (raw, expected) in cases {
            assert_eq!(bool::parse(raw), expected, "raw {raw:?}");
        }
    }

    #[test]
    fn integers_parse_decimal_and_hexadecimal() {
        let cases = [
            (Some("60"), Some(60)),
            (Some(" -12 "), Some(-12)),
            (Some("+7"), Some(7)),
            (Some("2147483647"), Some(i32::MAX)),
            (Some("#ff"), Some(255)),
            (Some("0x10"), Some(16)),
            (Some("0X10"), Some(16)),
            (Some("&h1A"), Some(26)),
            (Some("&HFFFFFFFF"), Some(-1)),
            (None, Some(0)),
            (Some("2147483648"), None),
            (Some("2.5"), None),
            (Some("1,000"), None),
            (Some("0x"), None),
            (Some("0x+1"), None),
            (Some("ten"), None),
            (Some(""), None),
        ];
        for (raw, expected) in cases {
            assert_eq!(i32::parse(raw), expected, "raw {raw:?}");
        }
    }

    #[test]
    fn strings_keep_the_stored_text() {
        assert_eq!(String::parse(Some(" a ")).as_deref(), Some(" a "));
        assert_eq!(String::parse(None).as_deref(), Some(""));
        assert_eq!(<Option<String>>::parse(Some("")), Some(Some(String::new())));
        assert_eq!(<Option<String>>::parse(None), Some(None));
    }

    #[test]
    fn enums_parse_names_and_numbers() {
        assert_eq!(LogLevel::parse(Some("Error")), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse(Some("3")), Some(LogLevel::Warning));
        assert_eq!(LogLevel::parse(None), Some(LogLevel::Verbose));
        assert_eq!(
            DownloadClient::parse(Some("Bezzad")),
            Some(DownloadClient::Internal)
        );
        assert_eq!(Provider::parse(Some("HIGHWAY")), Some(Provider::Highway));
        assert_eq!(
            TorrentFinishedAction::parse(Some("RemoveRealDebrid")),
            Some(TorrentFinishedAction::RemoveProvider)
        );
        assert_eq!(LogLevel::parse(Some("error")), None);
        assert_eq!(LogLevel::parse(Some("9")), None);
        assert_eq!(LogLevel::parse(Some("")), None);
    }

    #[test]
    fn values_reflect_the_field_type() {
        assert_eq!(true.to_value(), SettingValue::Boolean(true));
        assert_eq!(5.to_value(), SettingValue::Int32(5));
        assert_eq!(LogLevel::Error.to_value(), SettingValue::Enum(4));
        assert_eq!(
            "x".to_owned().to_value(),
            SettingValue::String(Some("x".to_owned()))
        );
        assert_eq!(None::<String>.to_value(), SettingValue::String(None));
    }
}

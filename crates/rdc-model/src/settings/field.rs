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

    /// The current value.
    fn to_value(&self) -> SettingValue;
}

impl Field for bool {
    const KIND: SettingKind = SettingKind::Boolean;

    fn to_value(&self) -> SettingValue {
        SettingValue::Boolean(*self)
    }
}

impl Field for i32 {
    const KIND: SettingKind = SettingKind::Int32;

    fn to_value(&self) -> SettingValue {
        SettingValue::Int32(*self)
    }
}

impl Field for String {
    const KIND: SettingKind = SettingKind::String;

    fn to_value(&self) -> SettingValue {
        SettingValue::String(Some(self.clone()))
    }
}

impl Field for Option<String> {
    const KIND: SettingKind = SettingKind::String;

    fn to_value(&self) -> SettingValue {
        SettingValue::String(self.clone())
    }
}

/// Implements [`Field`] for enums.
macro_rules! enum_field {
    ($($enum:ty),+ $(,)?) => {
        $(
            impl Field for $enum {
                const KIND: SettingKind = SettingKind::Enum;
                const OPTIONS: &'static [(i32, &'static str)] = <$enum>::OPTIONS;

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

/// Connects the fields of a settings group to their catalogue entries.
///
/// Generates, in declaration order, the group's `LEAVES` metadata plus
/// `leaf_value` dispatching on the setting name.
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
        }
    };
}

pub(crate) use section;

#[cfg(test)]
mod tests {
    use super::*;

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

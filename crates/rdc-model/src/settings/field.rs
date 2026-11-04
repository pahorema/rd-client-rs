//! Parsing of stored setting text into typed fields, and the macro that
//! connects a settings group's fields to their catalogue keys.

use super::value::SettingKind;
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
}

impl Field for bool {
    const KIND: SettingKind = SettingKind::Boolean;
}

impl Field for i32 {
    const KIND: SettingKind = SettingKind::Int32;
}

impl Field for String {
    const KIND: SettingKind = SettingKind::String;
}

impl Field for Option<String> {
    const KIND: SettingKind = SettingKind::String;
}

/// Implements [`Field`] for enums.
macro_rules! enum_field {
    ($($enum:ty),+ $(,)?) => {
        $(
            impl Field for $enum {
                const KIND: SettingKind = SettingKind::Enum;
                const OPTIONS: &'static [(i32, &'static str)] = <$enum>::OPTIONS;
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
/// Generates the group's `LEAVES` metadata in declaration order.
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
        }
    };
}

pub(crate) use section;

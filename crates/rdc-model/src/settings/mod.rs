//! Application settings: the typed tree, the catalogue shown by the web UI
//! and the conversions to and from the `Settings` table rows.

mod value;

pub use value::{SettingKind, SettingValue, stored_text_from_json};

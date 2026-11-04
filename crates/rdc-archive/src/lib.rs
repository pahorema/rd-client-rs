//! RAR and ZIP archive handling for rd-client.
//!
//! Provides helpers to recognise the volumes of multi-volume archive sets.

mod volume;

pub use volume::{first_volume, is_non_first_volume, volume_paths};

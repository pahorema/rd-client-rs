//! RAR and ZIP archive handling for rd-client.
//!
//! Provides helpers to recognise the volumes of multi-volume archive sets.

mod error;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by archive extraction, added next")
)]
mod path;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by archive extraction, added next")
)]
mod progress;
mod volume;

pub use error::Error;
pub use volume::{first_volume, is_non_first_volume, volume_paths};

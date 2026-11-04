//! RAR and ZIP archive handling for rd-client.
//!
//! [`list_entries`] lists the files of an archive. The volume helpers recognise the parts of
//! multi-volume archive sets by name.

mod error;
mod extract;
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
mod zip_format;

pub use error::Error;
pub use extract::{Entry, list_entries};
pub use volume::{first_volume, is_non_first_volume, volume_paths};

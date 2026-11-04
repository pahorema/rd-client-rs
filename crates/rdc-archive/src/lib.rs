//! RAR and ZIP archive handling for rd-client.
//!
//! [`extract`] unpacks an archive into a directory with progress reporting, cancellation and
//! protection against entries escaping the destination; [`list_entries`] lists its files. The
//! volume helpers recognise the parts of multi-volume archive sets by name.

mod error;
mod extract;
mod path;
mod progress;
mod rar_format;
mod volume;
mod zip_format;

pub use error::Error;
pub use extract::{Entry, extract, list_entries};
pub use volume::{first_volume, is_non_first_volume, volume_paths};

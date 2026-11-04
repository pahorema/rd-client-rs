//! RAR and ZIP archive handling for rd-client.
//!
//! [`unpack`] runs the procedure applied to a finished archive download: extract it next to
//! the torrent's files, then delete it. [`extract`] unpacks an archive into a directory with
//! progress reporting, cancellation and protection against entries escaping the destination;
//! [`list_entries`] lists its files. The volume helpers recognise the parts of multi-volume
//! archive sets by name, so callers can wait for a whole set and skip its later volumes.
//!
//! A multi-volume RAR set is unpacked from its first volume and needs every volume on disk:
//! callers must hold back [`unpack`] for a set until all of its volumes have finished
//! downloading (downloads of one set share the same [`first_volume`]). Unpacking a later volume
//! does nothing, and a set with a missing volume fails with [`Error::Read`].

mod error;
mod extract;
mod path;
mod progress;
mod rar_format;
mod unpack;
mod volume;
mod zip_format;

pub use error::Error;
pub use extract::{Entry, extract, list_entries};
pub use unpack::{UnpackRequest, unpack};
pub use volume::{first_volume, is_non_first_volume, volume_paths};

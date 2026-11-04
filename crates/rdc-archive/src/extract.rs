//! Format detection, entry listing and extraction.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::{Error, rar_format, zip_format};

/// A file stored in an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Entry {
    /// Name as stored in the archive, with `/` or `\` separating its components.
    pub name: String,
    /// Uncompressed size in bytes.
    pub size: u64,
}

/// Archive formats recognised by their signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Zip,
    Rar,
}

/// Number of leading bytes needed to recognise every supported signature.
const SIGNATURE_LEN: u64 = 6;

fn detect(archive: &Path) -> Result<Format, Error> {
    let file = File::open(archive).map_err(Error::io("open", archive))?;
    let mut signature = Vec::new();
    file.take(SIGNATURE_LEN)
        .read_to_end(&mut signature)
        .map_err(Error::io("read", archive))?;
    match signature.as_slice() {
        [b'P', b'K', 3 | 5 | 7, ..] => Ok(Format::Zip),
        [b'R', b'a', b'r', b'!', 0x1A, 0x07] => Ok(Format::Rar),
        _ => Err(Error::UnsupportedFormat {
            path: archive.to_path_buf(),
        }),
    }
}

/// Lists the file entries (directories excluded) of the ZIP or RAR archive at `archive`.
///
/// The format is recognised from the file's signature, not its extension. A RAR volume lists
/// the entries of the volumes following it too. This call blocks; run it on a blocking thread
/// from async code.
pub fn list_entries(archive: &Path) -> Result<Vec<Entry>, Error> {
    match detect(archive)? {
        Format::Zip => zip_format::list(archive),
        Format::Rar => rar_format::list(archive),
    }
}

/// Extracts the ZIP or RAR archive at `archive` into `destination`.
///
/// For a multi-volume RAR set, pass its first volume: the following volumes are read from the
/// same directory and a missing one fails the extraction. Directories are created as needed
/// and existing files are replaced. Entries that would end up outside `destination` (absolute
/// names, `..` escapes, paths through symbolic links) make the extraction fail with
/// [`Error::UnsafeEntry`] before anything is written. `on_progress` receives the percentage of
/// uncompressed bytes written so far, each value once and in increasing order, ending with 100
/// on success.
///
/// `cancel` is checked between entries, and for ZIP archives also while an entry is written; a
/// RAR entry, once started, is written to the end. A cancelled extraction fails with
/// [`Error::Cancelled`] and leaves the files written so far.
///
/// This call blocks; run it on a blocking thread from async code.
pub fn extract<F>(
    archive: &Path,
    destination: &Path,
    mut on_progress: F,
    cancel: &CancellationToken,
) -> Result<(), Error>
where
    F: FnMut(u8) + Send,
{
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    match detect(archive)? {
        Format::Zip => zip_format::extract(archive, destination, &mut on_progress, cancel),
        Format::Rar => rar_format::extract(archive, destination, &mut on_progress, cancel),
    }
}

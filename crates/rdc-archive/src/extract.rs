//! Format detection, entry listing and extraction.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::{Error, rar_format, volume_paths, zip_format};

/// A file stored in an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Entry {
    /// Name as stored in the archive. RAR names use `/` between components on every platform;
    /// ZIP names are kept verbatim and may use `\` when the archive was written that way.
    pub name: String,
    /// Uncompressed size in bytes.
    pub size: u64,
}

/// Position of an opened archive in a multi-volume set, as recorded in its headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VolumeRole {
    /// A complete archive on its own.
    Single,
    /// The first volume of a set.
    First,
    /// A volume of a set other than the first.
    Subsequent,
}

/// File entries of an archive and its position in a volume set.
#[derive(Debug)]
pub(crate) struct Inspection {
    pub(crate) entries: Vec<Entry>,
    pub(crate) volume: VolumeRole,
}

/// Archive formats recognised by their signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Zip,
    Rar,
}

/// Number of leading bytes needed to recognise every supported signature.
const SIGNATURE_LEN: u64 = 6;

/// Largest ratio between the uncompressed size of an archive and its size on disk.
///
/// Media barely compresses; 100 leaves ample room for text-heavy or sparse content, while a
/// decompression bomb can write at most 100 times what was downloaded.
const MAX_EXPANSION: u64 = 100;

/// Uncompressed size always allowed whatever the ratio, so small but highly compressible
/// archives (a disk image of zeros, logs) still unpack.
const MIN_UNPACKED_LIMIT: u64 = 1024 * 1024 * 1024;

/// Largest uncompressed size allowed for `archive`, together with the following volumes of its
/// set: [`MAX_EXPANSION`] times their size on disk, but at least [`MIN_UNPACKED_LIMIT`].
fn unpacked_limit(archive: &Path) -> u64 {
    let packed = volume_paths(archive)
        .iter()
        .filter_map(|volume| fs::metadata(volume).ok())
        .fold(0_u64, |sum, metadata| sum.saturating_add(metadata.len()));
    packed.saturating_mul(MAX_EXPANSION).max(MIN_UNPACKED_LIMIT)
}

/// Fails with [`Error::TooLarge`] when the archive declares more than `limit` uncompressed bytes.
pub(crate) fn ensure_within_limit(unpacked: u64, limit: u64) -> Result<(), Error> {
    if unpacked > limit {
        return Err(Error::TooLarge { unpacked, limit });
    }
    Ok(())
}

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
    inspect(archive).map(|inspection| inspection.entries)
}

/// Lists the file entries of `archive` and tells which volume of a set it is.
pub(crate) fn inspect(archive: &Path) -> Result<Inspection, Error> {
    match detect(archive)? {
        Format::Zip => Ok(Inspection {
            entries: zip_format::list(archive)?,
            volume: VolumeRole::Single,
        }),
        Format::Rar => rar_format::inspect(archive),
    }
}

/// Extracts the ZIP or RAR archive at `archive` into `destination`.
///
/// For a multi-volume RAR set, pass its first volume: the following volumes are read from the
/// same directory and a missing one fails the extraction. Directories are created as needed
/// and existing files are replaced.
///
/// The extraction is refused before anything is written for entries that would end up outside
/// `destination` (absolute names, `..` escapes, paths through symbolic links), with
/// [`Error::UnsafeEntry`]; for RAR entries that are links or copies of other files (symbolic and
/// hard links, junctions, file copies), encrypted, or without a recorded size, with
/// [`Error::UnsupportedEntry`]; and for an archive declaring more uncompressed data than 100
/// times its size on disk (1 GiB is always allowed), with [`Error::TooLarge`].
///
/// `on_progress` receives the percentage of uncompressed bytes written so far, each value once
/// and in increasing order, ending with 100 on success.
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
    let limit = unpacked_limit(archive);
    match detect(archive)? {
        Format::Zip => zip_format::extract(archive, destination, limit, &mut on_progress, cancel),
        Format::Rar => rar_format::extract(archive, destination, limit, &mut on_progress, cancel),
    }
}

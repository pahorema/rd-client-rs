//! Format detection, entry listing and extraction.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::{Error, zip_format};

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
}

/// Number of leading bytes needed to recognise every supported signature.
const SIGNATURE_LEN: u64 = 4;

fn detect(archive: &Path) -> Result<Format, Error> {
    let file = File::open(archive).map_err(Error::io("open", archive))?;
    let mut signature = Vec::new();
    file.take(SIGNATURE_LEN)
        .read_to_end(&mut signature)
        .map_err(Error::io("read", archive))?;
    match signature.as_slice() {
        [b'P', b'K', 3 | 5 | 7, ..] => Ok(Format::Zip),
        _ => Err(Error::UnsupportedFormat {
            path: archive.to_path_buf(),
        }),
    }
}

/// Lists the file entries (directories excluded) of the archive at `archive`.
///
/// The format is recognised from the file's signature, not its extension. This call blocks;
/// run it on a blocking thread from async code.
pub fn list_entries(archive: &Path) -> Result<Vec<Entry>, Error> {
    match detect(archive)? {
        Format::Zip => zip_format::list(archive),
    }
}

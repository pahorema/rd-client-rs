//! RAR listing and extraction through the bundled `UnRAR` C++ library.
//!
//! The library follows multi-volume sets on its own: opening the first volume reads the following
//! ones (`.partN.rar`, `.rNN` or `.NNN` names) as their content is needed and fails when one is
//! missing.

use std::io;
use std::path::Path;

use tokio_util::sync::CancellationToken;
use unrar_ng::Archive;

use crate::Error;
use crate::extract::Entry;
use crate::path::{create_entry_directory, entry_destination, prepare_file_target};
use crate::progress::Progress;

/// The library takes C strings; a path with a NUL byte cannot name a file anyway.
fn ensure_no_nul(path: &Path) -> Result<(), Error> {
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(Error::io("open", path)(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains a NUL byte",
        )));
    }
    Ok(())
}

/// Lists the file entries of `archive` (and of its following volumes), skipping directories.
pub(crate) fn list(archive: &Path) -> Result<Vec<Entry>, Error> {
    ensure_no_nul(archive)?;
    let listing = Archive::new(archive)
        .open_for_listing()
        .map_err(Error::read(archive))?;
    let mut entries = Vec::new();
    for header in listing {
        let header = header.map_err(Error::read(archive))?;
        if header.is_file() {
            entries.push(Entry {
                name: header.filename.to_string_lossy().into_owned(),
                size: header.unpacked_size,
            });
        }
    }
    Ok(entries)
}

/// Validates every entry name against `destination` and returns the total uncompressed size.
fn scan(archive: &Path, destination: &Path) -> Result<u64, Error> {
    let listing = Archive::new(archive)
        .open_for_listing()
        .map_err(Error::read(archive))?;
    let mut total = 0_u64;
    for header in listing {
        let header = header.map_err(Error::read(archive))?;
        entry_destination(destination, &header.filename.to_string_lossy())?;
        if header.is_file() {
            total = total.saturating_add(header.unpacked_size);
        }
    }
    Ok(total)
}

/// Extracts every entry of `archive` (and of its following volumes) below `destination`.
///
/// All entry names are validated before anything is written. Cancellation is checked between
/// entries: the library writes one entry in a single call that cannot be interrupted.
pub(crate) fn extract(
    archive: &Path,
    destination: &Path,
    on_progress: &mut (dyn FnMut(u8) + Send),
    cancel: &CancellationToken,
) -> Result<(), Error> {
    ensure_no_nul(archive)?;
    ensure_no_nul(destination)?;
    let total = scan(archive, destination)?;

    let mut progress = Progress::new(total, on_progress);
    let mut open = Archive::new(archive)
        .open_for_processing()
        .map_err(Error::read(archive))?;
    while let Some(header) = open.read_header().map_err(Error::read(archive))? {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let entry = header.entry();
        let name = entry.filename.to_string_lossy().into_owned();
        let target = entry_destination(destination, &name)?;
        if entry.is_directory() {
            create_entry_directory(destination, &target, &name)?;
            open = header.skip().map_err(Error::read(archive))?;
            continue;
        }
        prepare_file_target(destination, &target, &name)?;
        let size = entry.unpacked_size;
        open = header.extract_to(&target).map_err(Error::read(archive))?;
        progress.advance(size);
    }
    progress.finish();
    Ok(())
}

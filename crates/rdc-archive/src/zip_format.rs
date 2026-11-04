//! ZIP listing and extraction.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;

use tokio_util::sync::CancellationToken;
use zip::ZipArchive;

use crate::Error;
use crate::extract::Entry;
use crate::path::{create_entry_directory, entry_destination, prepare_file_target};
use crate::progress::Progress;

/// Size of the copy buffer; cancellation and progress are checked once per buffer.
const COPY_BUFFER: usize = 256 * 1024;

type Zip = ZipArchive<BufReader<File>>;

fn open(archive: &Path) -> Result<Zip, Error> {
    let file = File::open(archive).map_err(Error::io("open", archive))?;
    ZipArchive::new(BufReader::new(file)).map_err(Error::read(archive))
}

/// Lists the file entries of `archive`, skipping directories.
pub(crate) fn list(archive: &Path) -> Result<Vec<Entry>, Error> {
    let mut zip = open(archive)?;
    let mut entries = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let file = zip.by_index_raw(index).map_err(Error::read(archive))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().map_err(Error::read(archive))?.into_owned();
        entries.push(Entry {
            name,
            size: file.size(),
        });
    }
    Ok(entries)
}

/// Extracts every entry of `archive` below `destination`.
///
/// All entry names are validated before anything is written, so an archive with an escaping
/// entry leaves the destination untouched.
pub(crate) fn extract(
    archive: &Path,
    destination: &Path,
    on_progress: &mut (dyn FnMut(u8) + Send),
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let mut zip = open(archive)?;
    let mut total = 0_u64;
    for index in 0..zip.len() {
        let file = zip.by_index_raw(index).map_err(Error::read(archive))?;
        entry_destination(destination, &file.name().map_err(Error::read(archive))?)?;
        if !file.is_dir() {
            total = total.saturating_add(file.size());
        }
    }

    let mut progress = Progress::new(total, on_progress);
    let mut buffer = vec![0; COPY_BUFFER];
    for index in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut file = zip.by_index(index).map_err(Error::read(archive))?;
        let name = file.name().map_err(Error::read(archive))?.into_owned();
        let target = entry_destination(destination, &name)?;
        if file.is_dir() {
            create_entry_directory(destination, &target, &name)?;
            continue;
        }
        prepare_file_target(destination, &target, &name)?;
        let mut output = File::create(&target).map_err(Error::io("create file", &target))?;
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let read = file.read(&mut buffer).map_err(Error::read(archive))?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .map_err(Error::io("write", &target))?;
            progress.advance(u64::try_from(read).unwrap_or(u64::MAX));
        }
    }
    progress.finish();
    Ok(())
}

//! RAR listing and extraction through the bundled `UnRAR` C++ library.
//!
//! The library follows multi-volume sets on its own: opening the first volume reads the following
//! ones (`.partN.rar`, `.rNN` or `.NNN` names) as their content is needed and fails when one is
//! missing.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use unrar_ng::{
    Archive, CursorBeforeFile, CursorBeforeHeader, FileHeader, OpenArchive, Process, Redirection,
    UnrarResult, VolumeInfo,
};

use crate::Error;
use crate::extract::{Entry, Inspection, VolumeRole, ensure_within_limit};
use crate::path::{create_entry_directory, entry_destination, prepare_file_target};
use crate::progress::Progress;

/// Entries at least this large are watched while the library writes them, so progress keeps moving
/// during a long write instead of jumping once the entry is done.
const WATCHED_ENTRY_SIZE: u64 = 8 * 1024 * 1024;

/// Interval between two size checks of a watched entry.
const WATCH_INTERVAL: Duration = Duration::from_millis(250);

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

/// The name of an entry with `/` between its components on every platform; the library reports
/// names with the host's separator.
fn entry_name(path: &Path) -> String {
    let name = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        name.into_owned()
    } else {
        name.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

/// Refuses entries the library would not write from their own data, or could not write at all.
///
/// Links and file copies are never extracted: the library resolves the source of a hard link or
/// file copy against the process's working directory, so such an entry could copy any server file
/// into the destination, and a symbolic link could point anywhere. Encrypted entries need a
/// password the unpack never has; refusing them up front avoids a half-written destination.
fn ensure_extractable(header: &FileHeader, name: &str) -> Result<(), Error> {
    let reason = match header.redirection {
        Redirection::None if header.is_encrypted() => "it is encrypted",
        Redirection::None => return Ok(()),
        Redirection::UnixSymlink | Redirection::WindowsSymlink => "it is a symbolic link",
        Redirection::Junction => "it is a directory junction",
        Redirection::HardLink => "it is a hard link",
        Redirection::FileCopy => "it is a copy of another entry",
        _ => "it is a link to another file",
    };
    Err(Error::UnsupportedEntry {
        entry: name.to_owned(),
        reason,
    })
}

/// Lists the file entries of `archive` (and of its following volumes), skipping directories,
/// and tells which volume of a set `archive` is.
pub(crate) fn inspect(archive: &Path) -> Result<Inspection, Error> {
    ensure_no_nul(archive)?;
    let listing = Archive::new(archive)
        .open_for_listing()
        .map_err(Error::read(archive))?;
    let volume = match listing.volume_info() {
        VolumeInfo::None => VolumeRole::Single,
        VolumeInfo::First => VolumeRole::First,
        VolumeInfo::Subsequent => VolumeRole::Subsequent,
    };
    let mut entries = Vec::new();
    for header in listing {
        let header = header.map_err(Error::read(archive))?;
        if header.is_file() {
            entries.push(Entry {
                name: entry_name(&header.filename),
                size: header.unpacked_size,
            });
        }
    }
    Ok(Inspection { entries, volume })
}

/// The unpacked size the library reports for an entry whose header says it is unknown
/// (`INT64NDF` in the library's `rartypes.hpp`): such an entry is decompressed until its data
/// ends, whatever size that turns out to be.
const UNKNOWN_SIZE: u64 = 0x7fff_ffff_7fff_ffff;

/// Validates every entry against `destination` and returns the total uncompressed size.
fn scan(archive: &Path, destination: &Path) -> Result<u64, Error> {
    let listing = Archive::new(archive)
        .open_for_listing()
        .map_err(Error::read(archive))?;
    let mut total = 0_u64;
    for header in listing {
        let header = header.map_err(Error::read(archive))?;
        let name = header.filename.to_string_lossy();
        entry_destination(destination, &name)?;
        ensure_extractable(&header, &name)?;
        if header.is_file() {
            if header.unpacked_size == UNKNOWN_SIZE {
                return Err(Error::UnsupportedEntry {
                    entry: name.into_owned(),
                    reason: "its unpacked size is not recorded",
                });
            }
            total = total.saturating_add(header.unpacked_size);
        }
    }
    Ok(total)
}

/// Extracts every entry of `archive` (and of its following volumes) below `destination`.
///
/// All entries, and their total uncompressed size against `limit`, are validated before anything
/// is written. Cancellation is checked between entries: the library writes one entry in a single
/// call that cannot be interrupted.
pub(crate) fn extract(
    archive: &Path,
    destination: &Path,
    limit: u64,
    on_progress: &mut (dyn FnMut(u8) + Send),
    cancel: &CancellationToken,
) -> Result<(), Error> {
    ensure_no_nul(archive)?;
    ensure_no_nul(destination)?;
    let total = scan(archive, destination)?;
    ensure_within_limit(total, limit)?;

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
        ensure_extractable(entry, &name)?;
        if entry.is_directory() {
            create_entry_directory(destination, &target, &name)?;
            open = header.skip().map_err(Error::read(archive))?;
            continue;
        }
        prepare_file_target(destination, &target, &name)?;
        let size = entry.unpacked_size;
        let start = progress.done();
        open = if size >= WATCHED_ENTRY_SIZE {
            extract_watched(header, &target, size, &mut progress)
        } else {
            header.extract_to(&target)
        }
        .map_err(Error::read(archive))?;
        progress.advance_to(start.saturating_add(size));
    }
    progress.finish();
    Ok(())
}

/// Extracts the entry under `header` to `target` while a helper thread turns the growing size
/// of `target` into progress.
fn extract_watched(
    header: OpenArchive<Process, CursorBeforeFile>,
    target: &Path,
    size: u64,
    progress: &mut Progress<'_>,
) -> UnrarResult<OpenArchive<Process, CursorBeforeHeader>> {
    let start = progress.done();
    let progress = Mutex::new(progress);
    let finished = AtomicBool::new(false);
    thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                thread::park_timeout(WATCH_INTERVAL);
                if let Ok(metadata) = fs::metadata(target) {
                    let written = metadata.len().min(size);
                    progress
                        .lock()
                        .expect("progress lock is never poisoned")
                        .advance_to(start.saturating_add(written));
                }
            }
        });
        let result = header.extract_to(target);
        finished.store(true, Ordering::Release);
        watcher.thread().unpark();
        result
    })
}

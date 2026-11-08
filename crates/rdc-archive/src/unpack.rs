//! The unpack procedure run after a downloaded archive is complete.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::extract::{VolumeRole, extract, inspect};
use crate::path::{join_name, remove_with_retry};
use crate::volume::locate;
use crate::{Error, volume_paths};

/// Extension of the second volume of an old-style RAR set; an archive with an entry of this
/// extension wraps such a set.
const WRAPPED_SET_EXTENSION: &str = "r00";

/// What to unpack and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackRequest {
    /// The downloaded archive (a `.zip`, a `.rar` or the first volume of a RAR set).
    pub archive: PathBuf,
    /// The download directory of the torrent's category, i.e. the parent of the torrent folder.
    pub destination: PathBuf,
    /// The torrent name, as used for the torrent folder below `destination`.
    pub torrent_name: String,
    /// The torrent hash, for providers whose archives wrap the content in a `<hash>/<inner>/`
    /// folder pair that is flattened after extraction; `None` otherwise.
    pub flatten_hash_dir: Option<String>,
}

/// Unpacks a downloaded archive next to the torrent's other files, then deletes it.
///
/// The steps, in order:
/// 1. The archive is looked up ignoring the case of its volume part (`.rar`, `.part1.rar`), as
///    named by [`first_volume`](crate::first_volume). A missing archive file is not an error:
///    there is nothing to do (it was most likely unpacked and deleted before a restart). The
///    same holds for a RAR volume other than the first one of its set, which is unpacked
///    together with the first volume.
/// 2. When every entry sits below a folder named like the torrent (`<name>/…` or `<name>\…`),
///    the archive is extracted into `destination`; otherwise into `destination/<name>`, so no
///    entry can reach the files of other torrents in `destination`.
/// 3. When an entry has the `.r00` extension, the archive wraps an old-style RAR set: it is
///    extracted into a fresh temporary folder inside the extraction folder, then each wrapped
///    set (found through its `.r00` volume at the top of that folder) is extracted from its
///    `.rar` volume. Only then are the archive deleted and the temporary folder removed; when a
///    step fails, the temporary folder is removed and the archive kept, so a retry starts over.
///    Without any top-level `.r00` volume the temporary folder is kept, so its content is not
///    lost.
/// 4. Otherwise the archive is extracted and deleted. For a multi-volume set, its other
///    volumes are deleted as well.
/// 5. With [`flatten_hash_dir`](UnpackRequest::flatten_hash_dir) set and a `<hash>` folder in
///    the extraction folder, the content of its first sub-folder is moved up: into the
///    extraction folder when that is the torrent folder (step 2 chose `destination/<name>`),
///    otherwise into the `<hash>` folder. The emptied folders are removed.
///
/// `on_progress` receives an increasing percentage of the whole procedure, ending with 100 on
/// success. Deletes are retried a few times over several seconds before failing. The work runs
/// on a blocking thread.
///
/// # Errors
///
/// Fails when the archive cannot be read or extracted (see [`extract`](crate::extract)), when
/// `torrent_name` would leave `destination`, when a file cannot be deleted, or with
/// [`Error::Cancelled`] once `cancel` is triggered. Use [`Error::unpack_failure_message`] for the
/// message to show.
///
/// # Panics
///
/// Propagates a panic of the blocking task.
pub async fn unpack<F>(
    request: UnpackRequest,
    on_progress: F,
    cancel: CancellationToken,
) -> Result<(), Error>
where
    F: FnMut(u8) + Send + 'static,
{
    let task = tokio::task::spawn_blocking(move || unpack_blocking(&request, on_progress, &cancel));
    match task.await {
        Ok(result) => result,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(_) => Err(Error::Cancelled),
    }
}

fn unpack_blocking<F>(
    request: &UnpackRequest,
    on_progress: F,
    cancel: &CancellationToken,
) -> Result<(), Error>
where
    F: FnMut(u8) + Send,
{
    let UnpackRequest {
        archive,
        destination,
        torrent_name,
        flatten_hash_dir,
    } = request;
    let Some(archive) = locate(archive) else {
        debug!(archive = %archive.display(), "archive to unpack is gone, nothing to do");
        return Ok(());
    };
    let archive = archive.as_path();
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let inspection = inspect(archive)?;
    if inspection.volume == VolumeRole::Subsequent {
        info!(
            archive = %archive.display(),
            "not the first volume of its set, unpacked with the first volume"
        );
        return Ok(());
    }

    let rooted = !inspection.entries.is_empty()
        && inspection.entries.iter().all(|entry| {
            entry
                .name
                .strip_prefix(torrent_name.as_str())
                .is_some_and(|rest| rest.starts_with(['/', '\\']))
        });
    let extract_path = if rooted {
        destination.clone()
    } else {
        join_name(destination, torrent_name)?
    };

    let mut progress = Monotonic::new(on_progress);
    let wraps_set = inspection
        .entries
        .iter()
        .any(|entry| Path::new(&entry.name).extension() == Some(WRAPPED_SET_EXTENSION.as_ref()));
    if wraps_set {
        let temp = extract_path.join(Uuid::new_v4().to_string());
        fs::create_dir_all(&temp).map_err(Error::io("create directory", &temp))?;
        let wrapped = match unpack_wrapped(archive, &temp, &extract_path, &mut progress, cancel) {
            Ok(wrapped) => wrapped,
            Err(error) => {
                if let Err(cleanup) = fs::remove_dir_all(&temp) {
                    warn!(folder = %temp.display(), error = %cleanup, "temporary folder kept");
                }
                return Err(error);
            }
        };
        delete_archive(archive, inspection.volume)?;
        if !wrapped.is_empty() {
            remove_with_retry(&temp, |path| fs::remove_dir_all(path), "remove directory")?;
        }
    } else {
        extract(
            archive,
            &extract_path,
            |percent| progress.report(percent),
            cancel,
        )?;
        delete_archive(archive, inspection.volume)?;
    }
    if let Some(hash) = flatten_hash_dir {
        flatten(&extract_path, hash, !rooted)?;
    }
    progress.report(100);
    Ok(())
}

/// Extracts `archive` into `temp`, then each old-style set found at the top of `temp` into
/// `extract_path`, and returns the `.r00` volumes of the sets found; see step 3 of [`unpack`].
fn unpack_wrapped<F>(
    archive: &Path,
    temp: &Path,
    extract_path: &Path,
    progress: &mut Monotonic<F>,
    cancel: &CancellationToken,
) -> Result<Vec<PathBuf>, Error>
where
    F: FnMut(u8) + Send,
{
    extract(
        archive,
        temp,
        |percent| progress.report(percent / 2),
        cancel,
    )?;
    let wrapped = wrapped_sets(temp)?;
    for second_volume in &wrapped {
        let first_volume = second_volume.with_extension("rar");
        if first_volume.is_file() {
            extract(
                &first_volume,
                extract_path,
                |percent| progress.report(50 + percent / 2),
                cancel,
            )?;
        }
    }
    Ok(wrapped)
}

/// Moves the content of `<extract_path>/<hash>/<first sub-folder>/` up one or two levels, see
/// step 5 of [`unpack`]; `into_torrent_folder` tells whether `extract_path` is the torrent folder.
fn flatten(extract_path: &Path, hash: &str, into_torrent_folder: bool) -> Result<(), Error> {
    let hash_dir = join_name(extract_path, hash)?;
    if !hash_dir.is_dir() {
        return Ok(());
    }
    let Some(inner) = sorted_children(&hash_dir)?
        .into_iter()
        .find(|(_, is_dir)| *is_dir)
        .map(|(path, _)| path)
    else {
        debug!(folder = %hash_dir.display(), "hash folder has no sub-folder to flatten");
        return Ok(());
    };
    let target = if into_torrent_folder {
        extract_path
    } else {
        &hash_dir
    };
    for (child, _) in sorted_children(&inner)? {
        let Some(name) = child.file_name() else {
            continue;
        };
        let moved = target.join(name);
        if fs::symlink_metadata(&moved).is_ok() {
            return Err(Error::io("move to", &moved)(io::Error::from(
                io::ErrorKind::AlreadyExists,
            )));
        }
        fs::rename(&child, &moved).map_err(Error::io("move", &child))?;
    }
    fs::remove_dir(&inner).map_err(Error::io("remove directory", &inner))?;
    if into_torrent_folder && let Err(error) = fs::remove_dir(&hash_dir) {
        warn!(folder = %hash_dir.display(), %error, "hash folder kept after flattening");
    }
    Ok(())
}

/// Children of `dir` sorted by path, each with whether it is a directory (links excluded).
fn sorted_children(dir: &Path) -> Result<Vec<(PathBuf, bool)>, Error> {
    let mut children = Vec::new();
    for entry in fs::read_dir(dir).map_err(Error::io("list directory", dir))? {
        let entry = entry.map_err(Error::io("list directory", dir))?;
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        children.push((entry.path(), is_dir));
    }
    children.sort();
    Ok(children)
}

/// Top-level `.r00` files of `dir`, sorted.
fn wrapped_sets(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).map_err(Error::io("list directory", dir))? {
        let entry = entry.map_err(Error::io("list directory", dir))?;
        let path = entry.path();
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        if is_file && path.extension() == Some(WRAPPED_SET_EXTENSION.as_ref()) {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// Deletes the processed archive and, for the first volume of a set, the set's other volumes.
fn delete_archive(archive: &Path, volume: VolumeRole) -> Result<(), Error> {
    let mut paths = if volume == VolumeRole::First {
        volume_paths(archive)
    } else {
        Vec::new()
    };
    if !paths.iter().any(|path| path == archive) {
        paths.push(archive.to_path_buf());
    }
    for path in &paths {
        remove_with_retry(path, |path| fs::remove_file(path), "delete")?;
    }
    Ok(())
}

/// Forwards only percentages above the last one forwarded, so the phases of one procedure
/// add up to a single increasing progress.
struct Monotonic<F> {
    on_progress: F,
    last: Option<u8>,
}

impl<F: FnMut(u8)> Monotonic<F> {
    fn new(on_progress: F) -> Self {
        Self {
            on_progress,
            last: None,
        }
    }

    fn report(&mut self, percent: u8) {
        if self.last.is_none_or(|last| percent > last) {
            self.last = Some(percent);
            (self.on_progress)(percent);
        }
    }
}

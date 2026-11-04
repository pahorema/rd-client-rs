//! The symlink engine: links a file of a mounted provider drive into the download folder.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;

use crate::engine::{
    CANCELLED_MESSAGE, Completion, CompletionSender, DownloadEngine, Progress, StartError,
    completion_channel,
};

/// Search rounds before giving up.
const MAX_ATTEMPTS: u64 = 10;
/// Extensions the engine refuses: archives have to be downloaded and unpacked.
const COMPRESSED_EXTENSIONS: [&str; 3] = [".zip", ".rar", ".tar"];
/// Default wait unit between search rounds; round `n` waits `n` units.
const DEFAULT_RETRY_STEP: Duration = Duration::from_secs(1);

/// How the source file is located on the mount.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SourceLookup {
    /// Search the candidate directories of the mount.
    #[default]
    Search,
    /// Try `<mount>/<relative path>` first, then search; for providers whose mount layout
    /// matches the relative path exactly.
    DirectThenSearch,
}

/// What to link and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkRequest {
    /// Directory the provider drive is mounted at. A trailing `*` also searches its direct
    /// subdirectories.
    pub mount_path: String,
    /// Path of the file relative to the mount (usually `<torrent>/<sub dirs>/<file>`).
    pub relative_path: String,
    /// The link to create.
    pub destination: PathBuf,
    /// How the source file is located.
    pub lookup: SourceLookup,
}

/// Why linking failed; the message is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SymlinkError {
    /// The mount directory does not exist.
    #[error("Mount path {mount} does not exist!")]
    MountMissing {
        /// The mount directory, without trailing separators or `*`.
        mount: String,
    },
    /// The file is an archive.
    #[error("Cant handle compressed files with symlink downloader")]
    Compressed,
    /// The file was not found on the mount, or the link could not be created.
    #[error("Could not find file from rclone mount!")]
    NotFound,
}

/// Links the file of a mounted provider drive to the download path.
///
/// [`start`](DownloadEngine::start) does the whole job: it searches the mount for the file
/// (up to ten rounds, round `n` waiting `n` seconds first, about 45 s in total), creates the
/// link and returns the source path as remote id. Progress reports the round as
/// `{bytes_done: round, bytes_total: 10, speed: 1}`. Pausing does nothing; cancelling before
/// the start completes the engine.
#[derive(Debug)]
pub struct SymlinkDownload {
    request: SymlinkRequest,
    retry_step: Duration,
    progress: watch::Sender<Progress>,
    completion: CompletionSender,
    started: AtomicBool,
}

impl SymlinkDownload {
    /// Prepares linking as described by `request`.
    pub fn new(request: SymlinkRequest) -> Self {
        let (completion, _) = completion_channel();
        Self {
            request,
            retry_step: DEFAULT_RETRY_STEP,
            progress: watch::Sender::new(Progress::default()),
            completion,
            started: AtomicBool::new(false),
        }
    }

    /// Replaces the wait unit between search rounds (one second by default).
    #[must_use]
    pub fn with_retry_step(mut self, step: Duration) -> Self {
        self.retry_step = step;
        self
    }

    async fn link(&self) -> Result<PathBuf, SymlinkError> {
        let (mount, search_subdirectories) = parse_mount_path(&self.request.mount_path);
        if !is_dir(&mount).await {
            return Err(SymlinkError::MountMissing {
                mount: mount.display().to_string(),
            });
        }
        let relative = self.request.relative_path.as_str();
        let Some(file_name) = Path::new(relative)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            return Err(SymlinkError::NotFound);
        };
        let extension = Path::new(&file_name)
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy()))
            .unwrap_or_default();
        if COMPRESSED_EXTENSIONS.contains(&extension.as_str()) {
            return Err(SymlinkError::Compressed);
        }
        let stem = if extension.is_empty() {
            file_name.clone()
        } else {
            file_name.replace(&extension, "")
        };
        let parent = relative
            .strip_suffix(file_name.as_str())
            .unwrap_or(relative)
            .trim_end_matches(['\\', '/']);
        let search_path = mount.join(parent);
        self.progress.send_replace(Progress::default());

        let mut found = None;
        if self.request.lookup == SourceLookup::DirectThenSearch {
            let direct = mount.join(relative);
            if is_file(&direct).await {
                tracing::debug!("Found {relative} at {}", direct.display());
                found = Some(direct);
            } else {
                tracing::warn!(
                    "Expected {relative} at {}, but it is not there; searching the mount",
                    direct.display()
                );
            }
        }
        if found.is_none() {
            let candidates = candidate_directories(&mount, &search_path, &file_name, &stem);
            found = self
                .search(&mount, search_subdirectories, &candidates, &file_name)
                .await;
        }
        let Some(source) = found else {
            log_mount_contents(&mount).await;
            return Err(SymlinkError::NotFound);
        };
        tracing::debug!(
            "Linking {} to {}",
            self.request.destination.display(),
            source.display()
        );
        match create_link(&source, &self.request.destination).await {
            Ok(()) => {
                tracing::info!(
                    "Created symbolic link from {} to {}",
                    source.display(),
                    self.request.destination.display()
                );
                Ok(source)
            }
            Err(error) => {
                tracing::error!(
                    "Error creating symbolic link from {} to {}: {error}",
                    source.display(),
                    self.request.destination.display()
                );
                Err(SymlinkError::NotFound)
            }
        }
    }

    async fn search(
        &self,
        mount: &Path,
        search_subdirectories: bool,
        candidates: &[PathBuf],
        file_name: &str,
    ) -> Option<PathBuf> {
        for attempt in 0..MAX_ATTEMPTS {
            self.progress.send_replace(Progress {
                bytes_done: attempt,
                bytes_total: MAX_ATTEMPTS,
                speed: 1,
            });
            tracing::debug!(
                "Searching {} for {file_name} (attempt #{attempt})",
                mount.display()
            );
            if let Some(file) = find_file(mount, candidates, file_name).await {
                return Some(file);
            }
            if search_subdirectories {
                for subdirectory in subdirectories(mount).await {
                    if let Some(file) = find_file(&subdirectory, candidates, file_name).await {
                        return Some(file);
                    }
                }
            }
            let delay = self
                .retry_step
                .saturating_mul(u32::try_from(attempt).unwrap_or(u32::MAX));
            tokio::time::sleep(delay).await;
        }
        None
    }
}

#[async_trait]
impl DownloadEngine for SymlinkDownload {
    /// Finds the source file, links it and returns its path.
    async fn start(&self) -> Result<String, StartError> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Err(StartError::new("The download was already started"));
        }
        if self.completion.is_completed() {
            return Err(StartError::new(CANCELLED_MESSAGE));
        }
        tracing::debug!(
            "Resolving symlink source of {} for {}",
            self.request.relative_path,
            self.request.destination.display()
        );
        match self.link().await {
            Ok(source) => {
                self.completion.complete(Ok(()));
                Ok(source.to_string_lossy().into_owned())
            }
            Err(error) => {
                let message = error.to_string();
                self.completion.complete(Err(message.clone()));
                Err(StartError::new(message))
            }
        }
    }

    fn progress(&self) -> watch::Receiver<Progress> {
        self.progress.subscribe()
    }

    fn completion(&self) -> Completion {
        self.completion.subscribe()
    }

    /// Completes an engine that was not started; a running search finishes on its own.
    async fn cancel(&self) {
        if !self.started.load(Ordering::SeqCst) {
            self.completion.complete(Err(CANCELLED_MESSAGE.to_owned()));
        }
    }

    async fn pause(&self) {}

    async fn resume(&self) {}
}

/// Splits the configured mount path into the directory and whether to search one level of
/// subdirectories (trailing `*`).
fn parse_mount_path(configured: &str) -> (PathBuf, bool) {
    let trimmed = configured.trim_end_matches(['\\', '/']);
    let search_subdirectories = trimmed.ends_with('*');
    let mount = trimmed.trim_end_matches('*').trim_end_matches(['\\', '/']);
    (PathBuf::from(mount), search_subdirectories)
}

/// Directories (relative to a search root, or absolute) that may contain the file, in order:
/// the expected directory, the names of its ancestors below the mount (deepest first), a
/// directory named like the file, one named like the file without extension, and the root.
fn candidate_directories(
    mount: &Path,
    search_path: &Path,
    file_name: &str,
    stem: &str,
) -> Vec<PathBuf> {
    let mut candidates = vec![search_path.to_owned()];
    let mut directory = search_path.to_owned();
    while let Some(parent) = directory.parent() {
        let Some(name) = directory.file_name() else {
            break;
        };
        candidates.push(PathBuf::from(name));
        directory = parent.to_owned();
        if directory == mount {
            break;
        }
    }
    candidates.extend([
        PathBuf::from(file_name),
        PathBuf::from(stem),
        PathBuf::new(),
    ]);
    let mut distinct: Vec<PathBuf> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if !distinct.contains(&candidate) {
            distinct.push(candidate);
        }
    }
    distinct
}

async fn find_file(root: &Path, candidates: &[PathBuf], file_name: &str) -> Option<PathBuf> {
    for candidate in candidates {
        let path = root.join(candidate).join(file_name);
        tracing::trace!("Searching {}", path.display());
        if is_file(&path).await {
            return Some(path);
        }
    }
    None
}

/// Direct subdirectories of `directory`, sorted.
async fn subdirectories(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return found;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if is_dir(&path).await {
            found.push(path);
        }
    }
    found.sort();
    found
}

async fn log_mount_contents(mount: &Path) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    let mut names = Vec::new();
    match tokio::fs::read_dir(mount).await {
        Ok(mut entries) => {
            while let Ok(Some(entry)) = entries.next_entry().await {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Err(error) => {
            tracing::error!("Unable to list {}: {error}", mount.display());
            return;
        }
    }
    names.sort();
    tracing::debug!(
        "Unable to find the file in the mount. Entries of {}: {}",
        mount.display(),
        names.join(", ")
    );
}

/// Creates a symbolic link at `link` to `source`.
async fn create_link(source: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    tokio::fs::symlink(source, link).await?;
    #[cfg(windows)]
    tokio::fs::symlink_file(source, link).await?;
    // The link must resolve to the source.
    tokio::fs::metadata(link).await.map(|_| ())
}

async fn is_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
}

async fn is_dir(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_path_drops_trailing_separators() {
        assert_eq!(
            parse_mount_path("/mnt/rd/"),
            (PathBuf::from("/mnt/rd"), false)
        );
        assert_eq!(
            parse_mount_path("/mnt/rd"),
            (PathBuf::from("/mnt/rd"), false)
        );
    }

    #[test]
    fn trailing_star_enables_subdirectory_search() {
        assert_eq!(
            parse_mount_path("/mnt/rd/*"),
            (PathBuf::from("/mnt/rd"), true)
        );
        assert_eq!(
            parse_mount_path("/mnt/rd*/"),
            (PathBuf::from("/mnt/rd"), true)
        );
    }

    #[test]
    fn candidates_cover_expected_dir_ancestors_names_and_root() {
        let mount = Path::new("/mnt/rd");
        let candidates = candidate_directories(
            mount,
            &mount.join("Show/Season 1"),
            "episode.mkv",
            "episode",
        );
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/mnt/rd/Show/Season 1"),
                PathBuf::from("Season 1"),
                PathBuf::from("Show"),
                PathBuf::from("episode.mkv"),
                PathBuf::from("episode"),
                PathBuf::new(),
            ]
        );
    }

    #[test]
    fn candidates_are_distinct() {
        let mount = Path::new("/mnt/rd");
        let candidates = candidate_directories(mount, &mount.join("file"), "file", "file");
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/mnt/rd/file"),
                PathBuf::from("file"),
                PathBuf::new()
            ]
        );
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, b"x").expect("file");
    }

    #[tokio::test]
    async fn file_is_found_in_the_first_matching_candidate() {
        let root = tempfile::tempdir().expect("temp dir");
        let mount = root.path();
        let candidates = candidate_directories(mount, &mount.join("Show/Season 1"), "e.mkv", "e");
        assert_eq!(find_file(mount, &candidates, "e.mkv").await, None);
        touch(&mount.join("e.mkv"));
        assert_eq!(
            find_file(mount, &candidates, "e.mkv").await,
            Some(mount.join("e.mkv"))
        );
        touch(&mount.join("e/e.mkv"));
        assert_eq!(
            find_file(mount, &candidates, "e.mkv").await,
            Some(mount.join("e/e.mkv"))
        );
        touch(&mount.join("Show/e.mkv"));
        assert_eq!(
            find_file(mount, &candidates, "e.mkv").await,
            Some(mount.join("Show/e.mkv"))
        );
        touch(&mount.join("Show/Season 1/e.mkv"));
        assert_eq!(
            find_file(mount, &candidates, "e.mkv").await,
            Some(mount.join("Show/Season 1/e.mkv"))
        );
    }

    #[tokio::test]
    async fn directories_are_not_files() {
        let root = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(root.path().join("e.mkv")).expect("dir");
        assert!(!is_file(&root.path().join("e.mkv")).await);
        assert!(is_dir(&root.path().join("e.mkv")).await);
    }
}

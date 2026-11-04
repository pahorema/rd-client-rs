//! The symlink engine: links a file of a mounted provider drive into the download folder.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::watch;

use crate::engine::{
    CANCELLED_MESSAGE, Completion, CompletionSender, DownloadEngine, Progress, StartError,
    completion_channel,
};

/// What to link and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkRequest {
    /// Directory the provider drive is mounted at.
    pub mount_path: String,
    /// Path of the file relative to the mount (usually `<torrent>/<sub dirs>/<file>`).
    pub relative_path: String,
    /// The link to create.
    pub destination: PathBuf,
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
    /// The file was not found on the mount, or the link could not be created.
    #[error("Could not find file from rclone mount!")]
    NotFound,
}

/// Links the file of a mounted provider drive to the download path.
///
/// [`start`](DownloadEngine::start) does the whole job: it searches the mount for the file,
/// creates the link and returns the source path as remote id. Pausing does nothing; cancelling
/// before the start completes the engine.
#[derive(Debug)]
pub struct SymlinkDownload {
    request: SymlinkRequest,
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
            progress: watch::Sender::new(Progress::default()),
            completion,
            started: AtomicBool::new(false),
        }
    }

    async fn link(&self) -> Result<PathBuf, SymlinkError> {
        let mount = parse_mount_path(&self.request.mount_path);
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

        let candidates = candidate_directories(&mount, &search_path, &file_name, &stem);
        let found = find_file(&mount, &candidates, &file_name).await;
        let Some(source) = found else {
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

/// The mount directory of the configured mount path, without trailing separators.
fn parse_mount_path(configured: &str) -> PathBuf {
    PathBuf::from(configured.trim_end_matches(['\\', '/']))
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
        assert_eq!(parse_mount_path("/mnt/rd/"), PathBuf::from("/mnt/rd"));
        assert_eq!(parse_mount_path("/mnt/rd"), PathBuf::from("/mnt/rd"));
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

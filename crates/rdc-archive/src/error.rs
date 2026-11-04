//! Error type shared by extraction and the unpack procedure.

use std::io;
use std::path::{Path, PathBuf};

/// Failure while reading, extracting or unpacking an archive.
///
/// The `Display` text is suitable for showing to the user as the cause of a failed unpack.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file is neither a ZIP nor a RAR archive.
    #[error("{} is not a ZIP or RAR archive", path.display())]
    UnsupportedFormat {
        /// Path of the file.
        path: PathBuf,
    },
    /// The archive is damaged, truncated or uses an unsupported feature.
    #[error("cannot read archive {}: {source}", path.display())]
    Read {
        /// Path of the archive (or of the volume that was opened).
        path: PathBuf,
        /// Underlying error reported by the archive reader.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// An archive entry would be written outside the destination directory.
    #[error("archive entry {entry:?} escapes the destination directory")]
    UnsafeEntry {
        /// Entry name as stored in the archive.
        entry: String,
    },
    /// A file system operation failed.
    #[error("cannot {action} {}: {source}", path.display())]
    Io {
        /// What was being done, e.g. `"create directory"`.
        action: &'static str,
        /// Path the operation was applied to.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// The operation was cancelled through its cancellation token.
    #[error("the unpack was cancelled")]
    Cancelled,
}

impl Error {
    /// The message shown to the user when unpacking the download `link` of the torrent
    /// `torrent_name` failed with this error.
    pub fn unpack_failure_message(&self, link: Option<&str>, torrent_name: &str) -> String {
        let link = link.unwrap_or_default();
        format!("An unexpected error occurred unpacking {link} for torrent {torrent_name}: {self}")
    }

    /// Builds a closure mapping an [`io::Error`] to [`Error::Io`] for `path`.
    pub(crate) fn io(action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> Self {
        move |source| Self::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }

    /// Builds a closure mapping an archive reader error to [`Error::Read`] for `path`.
    pub(crate) fn read<E>(path: &Path) -> impl FnOnce(E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        move |source| Self::Read {
            path: path.to_path_buf(),
            source: Box::new(source),
        }
    }
}

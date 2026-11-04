//! Error type shared by extraction and the unpack procedure.

use std::io;
use std::path::{Path, PathBuf};

/// Failure while reading, extracting or unpacking an archive.
///
/// The `Display` text is suitable for showing to the user as the cause of a failed unpack.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
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
}

impl Error {
    /// Builds a closure mapping an [`io::Error`] to [`Error::Io`] for `path`.
    pub(crate) fn io(action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> Self {
        move |source| Self::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }
}

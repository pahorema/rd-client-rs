//! Errors returned by the store.

use std::{io, path::PathBuf};

use uuid::Uuid;

/// Failure of a store operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The directory that should contain the database file could not be created.
    #[error("cannot create the database directory {path}")]
    CreateDirectory {
        /// The directory that could not be created.
        path: PathBuf,
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },
    /// SQLite reported an error.
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    /// The database contains tables but no migration history, so its layout
    /// is unknown and it is left untouched.
    #[error("the database contains tables but no migration history")]
    UnversionedDatabase,
    /// A stored value could not be decoded.
    #[error("invalid value in column {column}: {value}")]
    InvalidValue {
        /// The column holding the value.
        column: &'static str,
        /// The stored value.
        value: String,
    },
    /// The settings table is empty; settings are seeded at startup.
    #[error("No settings found, please restart")]
    NoSettings,
    /// No download has this id.
    #[error("Cannot find download with ID {0}")]
    DownloadNotFound(Uuid),
}

/// Result of a store operation.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

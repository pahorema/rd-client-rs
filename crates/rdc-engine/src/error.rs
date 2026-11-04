//! User-visible failures of engine operations.

/// An engine operation could not be completed; its display text is shown to users.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// Invalid input or a lifecycle failure.
    #[error("{0}")]
    Message(String),
    /// Persistence operation failed.
    #[error(transparent)]
    Store(#[from] rdc_store::StoreError),
    /// Provider operation failed.
    #[error(transparent)]
    Provider(#[from] rdc_debrid::DebridError),
    /// Filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// HTTP client initialization or diagnostic request failed.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

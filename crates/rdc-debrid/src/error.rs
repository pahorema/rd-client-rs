//! Errors returned by debrid providers.

use std::time::Duration;

/// Failure of a debrid provider operation.
///
/// The `Display` text is shown to users (torrent errors, API responses), so it
/// is phrased as a complete, provider-specific message.
#[derive(Debug, thiserror::Error)]
pub enum DebridError {
    /// The provider asked to slow down; no request should be sent before `retry_after` elapses.
    #[error("{message}")]
    RateLimited {
        /// Human readable reason.
        message: String,
        /// How long to wait before the next request.
        retry_after: Duration,
    },
    /// The torrent or transfer no longer exists at the provider.
    #[error("{0}")]
    NotFound(String),
    /// The provider is not usable with the current settings (missing API key, bad hostname, ...).
    #[error("{0}")]
    NotConfigured(String),
    /// The provider rejected the request.
    #[error("{0}")]
    Provider(String),
    /// The provider answered with something that could not be understood.
    #[error("{0}")]
    InvalidResponse(String),
    /// The request could not be sent or the response could not be read.
    ///
    /// The text names the cause (timeout, refused connection, DNS or TLS
    /// failure) but never the request URL, whose query may carry an API key.
    /// The causes are part of the text, so the error reports no `source`.
    #[error("{}", transport_message(.0))]
    Transport(reqwest::Error),
    /// A torrent's include or exclude expression is invalid.
    #[error(transparent)]
    Filter(#[from] rdc_model::FilterError),
}

impl DebridError {
    /// Whether the provider reported the resource as gone.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }

    /// The cooldown requested by the provider, if this is a rate-limit error.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => Some(*retry_after),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for DebridError {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error.without_url())
    }
}

/// `error` followed by its causes, separated by `": "`; a cause whose text
/// the message already contains is skipped.
fn transport_message(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut cause = std::error::Error::source(error);
    while let Some(current) = cause {
        let text = current.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        cause = current.source();
    }
    message
}

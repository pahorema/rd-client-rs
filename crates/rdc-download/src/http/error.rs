//! Failures of the internal HTTP engine.

use std::error::Error as _;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use reqwest::StatusCode;

use crate::engine::CANCELLED_MESSAGE;

/// Why a transfer (or one attempt of a chunk) failed; the message is shown to the user.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TransferError {
    #[error(
        "Response status code does not indicate success: {} ({}).",
        .0.as_u16(),
        .0.canonical_reason().unwrap_or("Unknown")
    )]
    Status(StatusCode),
    // The message already carries the causes, so they are not repeated as a source.
    #[error("{}", describe(.0))]
    Http(reqwest::Error),
    #[error("No data was received for {} ms.", .0.as_millis())]
    ReadTimeout(Duration),
    #[error("The server did not respond within {} ms.", .0.as_millis())]
    HeaderTimeout(Duration),
    #[error("The download ended prematurely: received {received} of {expected} bytes of a chunk.")]
    Incomplete { received: u64, expected: u64 },
    #[error("The server ignored the requested byte range.")]
    RangeIgnored,
    #[error("The download was interrupted: {0}")]
    Interrupted(String),
    #[error("Invalid proxy server {proxy}: {}", describe(.error))]
    Proxy {
        proxy: String,
        error: reqwest::Error,
    },
    #[error("Unable to write {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{CANCELLED_MESSAGE}")]
    Cancelled,
}

impl TransferError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Whether the failure is a passing transport or server condition worth another try.
    pub(crate) fn is_transient(&self) -> bool {
        match self {
            Self::Status(status) => is_transient_status(*status),
            Self::Http(error) => {
                !error.is_builder() && error.status().is_none_or(is_transient_status)
            }
            Self::ReadTimeout(_) | Self::HeaderTimeout(_) => true,
            Self::Incomplete { .. }
            | Self::RangeIgnored
            | Self::Proxy { .. }
            | Self::Io { .. }
            | Self::Interrupted(_)
            | Self::Cancelled => false,
        }
    }

    /// Whether a chunk may retry after this failure (given the server serves ranges).
    pub(crate) fn is_retryable(&self) -> bool {
        self.is_transient() || matches!(self, Self::Incomplete { .. })
    }
}

/// Statuses that signal overload, throttling or a redirect rather than a permanent refusal.
pub(crate) fn is_transient_status(status: StatusCode) -> bool {
    matches!(
        status.as_u16(),
        300..=303 | 307 | 308 | 408 | 428 | 429 | 503 | 504
    )
}

/// Joins an error and its sources into one line.
fn describe(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_message_names_code_and_reason() {
        assert_eq!(
            TransferError::Status(StatusCode::NOT_FOUND).to_string(),
            "Response status code does not indicate success: 404 (Not Found)."
        );
    }

    #[test]
    fn overload_statuses_are_transient_and_client_errors_are_not() {
        for code in [408, 428, 429, 503, 504, 301, 302, 307] {
            let status = StatusCode::from_u16(code).expect("status");
            assert!(is_transient_status(status), "code {code}");
        }
        for code in [400, 401, 403, 404, 416, 500, 502] {
            let status = StatusCode::from_u16(code).expect("status");
            assert!(!is_transient_status(status), "code {code}");
        }
    }

    #[test]
    fn incomplete_chunks_are_retried_but_not_transient() {
        let error = TransferError::Incomplete {
            received: 1,
            expected: 2,
        };
        assert!(error.is_retryable());
        assert!(!error.is_transient());
        assert!(TransferError::ReadTimeout(Duration::from_secs(1)).is_retryable());
        assert!(TransferError::Status(StatusCode::SERVICE_UNAVAILABLE).is_transient());
        assert!(!TransferError::Status(StatusCode::NOT_FOUND).is_retryable());
        assert!(!TransferError::Cancelled.is_retryable());
    }

    #[test]
    fn cancellation_uses_the_cancelled_message() {
        assert_eq!(TransferError::Cancelled.to_string(), CANCELLED_MESSAGE);
    }
}

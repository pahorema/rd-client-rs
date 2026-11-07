//! Errors returned by the DSM client.

use std::fmt;

use crate::codes;

/// DSM error codes meaning the session is gone: 106 (timeout), 107 (interrupted by a
/// duplicate login) and 119 (SID not found). Logging in again usually fixes them.
pub const SESSION_ERROR_CODES: [i64; 3] = [106, 107, 119];

/// Failure of a DSM Web API call.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The DSM URL is blank or not an absolute HTTP(S) URL.
    #[error("invalid Synology URL {0:?}")]
    InvalidUrl(String),
    /// A required argument is missing or malformed; nothing was sent.
    #[error("{0}")]
    InvalidArgument(&'static str),
    /// The HTTP client could not be constructed.
    #[error("failed to build the HTTP client: {0}")]
    ClientBuild(#[source] reqwest::Error),
    /// The request could not be sent or the response could not be read. The request URL
    /// is stripped because its query carries the session id.
    // The message already carries the causes, so they are not repeated as a source.
    #[error("request to Synology failed: {}", describe(.0))]
    Transport(reqwest::Error),
    /// DSM answered with an HTTP status other than 200.
    #[error("received unexpected response status {0}")]
    UnexpectedStatus(u16),
    /// DSM answered `success: false`.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The response body is not the expected JSON shape.
    #[error("unable to parse the Synology response: {body}")]
    Decode {
        /// Parse failure.
        #[source]
        source: serde_json::Error,
        /// Raw response body.
        body: String,
    },
    /// A task lookup answered successfully but without the task.
    #[error("Download Station task {0} not found")]
    TaskNotFound(String),
}

impl Error {
    /// The DSM error code when DSM itself rejected the call.
    pub fn api_code(&self) -> Option<i64> {
        match self {
            Self::Api(error) => Some(error.code),
            _ => None,
        }
    }

    /// Whether DSM rejected the call because the session expired or was replaced
    /// (codes in [`SESSION_ERROR_CODES`]); a fresh login and one retry is the remedy.
    pub fn is_session_error(&self) -> bool {
        self.api_code()
            .is_some_and(|code| SESSION_ERROR_CODES.contains(&code))
    }
}

/// Joins an error and its sources into one line.
fn describe(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// Error reported by DSM in a `success: false` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// Name of the API that failed, for example `SYNO.API.Auth`.
    pub api: &'static str,
    /// Method that failed, for example `login`.
    pub method: &'static str,
    /// API version used for the call.
    pub version: u32,
    /// DSM error code.
    pub code: i64,
    /// Description of `code`; empty when the code is not documented.
    pub description: &'static str,
    /// Per-item details (for example one entry per path a file operation failed on).
    pub errors: Vec<ErrorDetail>,
}

/// One entry of the `errors` array of a DSM error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDetail {
    /// DSM error code for this item.
    pub code: i64,
    /// Description of `code`; empty when the code is not documented.
    pub description: &'static str,
    /// Path the error refers to, when given.
    pub path: Option<String>,
}

impl ApiError {
    /// An error with code `code` reported by `api`/`method`, described from the DSM
    /// documentation.
    pub fn new(api: &'static str, method: &'static str, version: u32, code: i64) -> Self {
        Self {
            api,
            method,
            version,
            code,
            description: codes::describe(api, code),
            errors: Vec::new(),
        }
    }

    /// Adds one per-item detail with code `code`, optionally about `path`.
    #[must_use]
    pub fn with_detail(mut self, code: i64, path: Option<String>) -> Self {
        self.errors.push(ErrorDetail {
            code,
            description: codes::describe(self.api, code),
            path,
        });
        self
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{} (version {}) failed with error {}",
            self.api, self.method, self.version, self.code
        )?;
        if !self.description.is_empty() {
            write!(f, ": {}", self.description)?;
        }
        for detail in &self.errors {
            write!(f, "; [{}]", detail.code)?;
            if !detail.description.is_empty() {
                write!(f, " {}", detail.description)?;
            }
            if let Some(path) = &detail.path {
                write!(f, " ({path})")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_codes_are_detected() {
        for code in [106, 107, 119] {
            let error = Error::from(ApiError::new("SYNO.DownloadStation2.Task", "list", 1, code));

            assert!(error.is_session_error(), "{code}");
        }
    }

    #[test]
    fn other_codes_are_not_session_errors() {
        let api = Error::from(ApiError::new("SYNO.DownloadStation2.Task", "list", 1, 105));
        let status = Error::UnexpectedStatus(502);

        assert!(!api.is_session_error());
        assert!(!status.is_session_error());
        assert_eq!(status.api_code(), None);
    }

    #[test]
    fn display_includes_api_code_description_and_details() {
        let error = ApiError::new("SYNO.FileStation.CreateFolder", "create", 2, 1100)
            .with_detail(408, Some("/test/:".to_owned()));

        assert_eq!(
            error.to_string(),
            "SYNO.FileStation.CreateFolder.create (version 2) failed with error 1100: Failed to create a folder. \
             More information in <errors> object; [408] No such file or directory (/test/:)"
        );
    }

    #[test]
    fn display_omits_an_unknown_description() {
        let error = ApiError::new("SYNO.DownloadStation.Info", "getconfig", 2, 499);

        assert_eq!(
            error.to_string(),
            "SYNO.DownloadStation.Info.getconfig (version 2) failed with error 499"
        );
    }
}

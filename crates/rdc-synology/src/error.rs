//! Errors returned by the DSM client.

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
    /// The request could not be sent or the response could not be read.
    #[error("request to Synology failed: {0}")]
    Transport(#[source] reqwest::Error),
    /// DSM answered with an HTTP status other than 200.
    #[error("received unexpected response status {0}")]
    UnexpectedStatus(u16),
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

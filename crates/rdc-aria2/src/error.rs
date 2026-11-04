//! Errors returned by the aria2 client.

/// Failure of an aria2 RPC call.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The configured RPC URL does not point at the JSON-RPC endpoint.
    #[error("The URL must end with /jsonrpc")]
    InvalidUrl,
    /// `aria2.addUri` was called without any URI.
    #[error("the URI list cannot be empty")]
    EmptyUriList,
    /// The HTTP client could not be constructed.
    #[error("failed to build the HTTP client: {0}")]
    ClientBuild(#[source] reqwest::Error),
    /// The request could not be sent or the response could not be read (after all retries).
    #[error("request to aria2 failed: {0}")]
    Transport(#[source] reqwest::Error),
    /// aria2 answered with a JSON-RPC error object.
    #[error("aria2 error {code}: {message}")]
    Rpc {
        /// Error code reported by aria2.
        code: i64,
        /// Error message reported by aria2.
        message: String,
    },
    /// aria2 answered with a non-success HTTP status and no JSON-RPC error object.
    #[error("unexpected HTTP status {status} from aria2: {body}")]
    Status {
        /// HTTP status code of the response.
        status: u16,
        /// Raw response body.
        body: String,
    },
    /// The response carried neither a result nor an error.
    #[error("no response received from aria2")]
    EmptyResponse,
    /// The response body is not the expected JSON shape.
    #[error("unable to parse the aria2 response: {body}")]
    Decode {
        /// Parse failure.
        #[source]
        source: serde_json::Error,
        /// Raw response body.
        body: String,
    },
}

impl Error {
    /// The aria2 error code when aria2 itself rejected the call.
    pub fn rpc_code(&self) -> Option<i64> {
        match self {
            Self::Rpc { code, .. } => Some(*code),
            _ => None,
        }
    }
}

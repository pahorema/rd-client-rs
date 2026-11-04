//! JSON-RPC transport and the typed aria2 calls.

use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

/// Request timeout used when [`ClientBuilder::timeout`] is not called.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Path suffix every aria2 RPC URL must end with.
const RPC_SUFFIX: &str = "/jsonrpc";

/// Identifier sent with every request; aria2 echoes it back and it is not otherwise used.
const REQUEST_ID: &str = "rd-client";

/// Configures and builds a [`Client`].
#[derive(Clone)]
#[must_use]
pub struct ClientBuilder {
    url: String,
    secret: Option<String>,
    timeout: Duration,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("url", &self.url)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl ClientBuilder {
    /// RPC secret (`--rpc-secret`). A blank secret means aria2 runs without one and no
    /// `token:` parameter is sent.
    pub fn secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = Some(secret.into());
        self
    }

    /// Timeout of each HTTP request.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Validates the configuration and builds the client.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidUrl`] when the URL does not end with `/jsonrpc`,
    /// [`Error::ClientBuild`] when the HTTP client cannot be created.
    pub fn build(self) -> Result<Client, Error> {
        if !self.url.ends_with(RPC_SUFFIX) {
            return Err(Error::InvalidUrl);
        }
        let http = reqwest::Client::builder()
            .user_agent(concat!("rd-client/", env!("CARGO_PKG_VERSION")))
            .timeout(self.timeout)
            .build()
            .map_err(Error::ClientBuild)?;
        let token = self
            .secret
            .filter(|secret| !secret.trim().is_empty())
            .map(|secret| format!("token:{secret}"));
        Ok(Client {
            http,
            url: self.url,
            token,
        })
    }
}

/// aria2 JSON-RPC client.
///
/// Cheap to clone; clones share the underlying connection pool.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    url: String,
    /// `token:<secret>`, present only when a non-blank secret is configured.
    token: Option<String>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

/// Version information returned by `aria2.getVersion`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    /// aria2 version number.
    pub version: String,
    /// Features compiled into this aria2 build.
    #[serde(default)]
    pub enabled_features: Vec<String>,
}

#[derive(Serialize)]
struct Request<'a> {
    jsonrpc: &'static str,
    id: &'static str,
    method: &'a str,
    params: &'a [Value],
}

#[derive(Deserialize)]
struct Response<T> {
    result: Option<T>,
    error: Option<RpcErrorBody>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: Option<RpcErrorBody>,
}

#[derive(Deserialize)]
struct RpcErrorBody {
    code: i64,
    message: Option<String>,
}

impl From<RpcErrorBody> for Error {
    fn from(body: RpcErrorBody) -> Self {
        Self::Rpc {
            code: body.code,
            message: body.message.unwrap_or_else(|| "Unknown error".to_owned()),
        }
    }
}

impl Client {
    /// Starts configuring a client for the RPC endpoint at `url`
    /// (for example `http://127.0.0.1:6800/jsonrpc`).
    pub fn builder(url: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            url: url.into(),
            secret: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// `aria2.getVersion`: the aria2 version and its enabled features.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the RPC round trip.
    pub async fn version(&self) -> Result<Version, Error> {
        self.call("aria2.getVersion", Vec::new()).await
    }

    /// Calls `method` with the secret token (when configured) followed by `args`.
    async fn call<T: DeserializeOwned>(&self, method: &str, args: Vec<Value>) -> Result<T, Error> {
        let body = self.post(method, &self.with_token(args)).await?;
        let response: Response<T> = decode(&body)?;
        if let Some(error) = response.error {
            return Err(error.into());
        }
        response.result.ok_or(Error::EmptyResponse)
    }

    fn with_token(&self, args: Vec<Value>) -> Vec<Value> {
        match &self.token {
            Some(token) => {
                let mut params = Vec::with_capacity(args.len() + 1);
                params.push(Value::String(token.clone()));
                params.extend(args);
                params
            }
            None => args,
        }
    }

    /// Posts one JSON-RPC request and returns the non-empty body of a successful response.
    async fn post(&self, method: &str, params: &[Value]) -> Result<String, Error> {
        let request = Request {
            jsonrpc: "2.0",
            id: REQUEST_ID,
            method,
            params,
        };
        let (status, body) = self.send(&request).await.map_err(Error::Transport)?;
        if !status.is_success() {
            if let Ok(ErrorResponse { error: Some(error) }) = serde_json::from_str(&body) {
                return Err(error.into());
            }
            return Err(Error::Status {
                status: status.as_u16(),
                body,
            });
        }
        if status == StatusCode::NO_CONTENT || body.is_empty() {
            return Err(Error::EmptyResponse);
        }
        Ok(body)
    }

    async fn send(&self, request: &Request<'_>) -> Result<(StatusCode, String), reqwest::Error> {
        let response = self.http.post(&self.url).json(request).send().await?;
        let status = response.status();
        let body = response.text().await?;
        Ok((status, body))
    }
}

fn decode<T: DeserializeOwned>(body: &str) -> Result<T, Error> {
    serde_json::from_str(body).map_err(|source| Error::Decode {
        source,
        body: body.to_owned(),
    })
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Client>();
    assert_send_sync::<Error>();
};

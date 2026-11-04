//! JSON-RPC transport and the typed aria2 calls.

use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{DownloadStatus, Error};

/// Request timeout used when [`ClientBuilder::timeout`] is not called.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How many stopped and how many waiting downloads [`Client::tell_all`] fetches.
pub const TELL_ALL_LIMIT: u32 = 1000;

/// Base delay between transport retries; the n-th retry waits `n` times this value.
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(1);

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
    retry_count: u32,
    retry_delay: Duration,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("url", &self.url)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("timeout", &self.timeout)
            .field("retry_count", &self.retry_count)
            .field("retry_delay", &self.retry_delay)
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

    /// How many times a request is retried after a transport failure (connection refused,
    /// timeout, truncated body). aria2 errors and HTTP error statuses are never retried.
    pub fn retry_count(mut self, retry_count: u32) -> Self {
        self.retry_count = retry_count;
        self
    }

    /// Base delay between retries; the n-th retry waits `n` times this value.
    pub fn retry_delay(mut self, retry_delay: Duration) -> Self {
        self.retry_delay = retry_delay;
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
            retry_count: self.retry_count,
            retry_delay: self.retry_delay,
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
    retry_count: u32,
    retry_delay: Duration,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("retry_count", &self.retry_count)
            .field("retry_delay", &self.retry_delay)
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

/// Options of a new download (`aria2.addUri`); unset options are not sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AddUriOptions {
    /// Directory on the aria2 host the file is saved to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// File name inside `dir`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
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

/// One entry of a `system.multicall` result: a one-element array holding the call's result,
/// or a fault object when that call failed.
#[derive(Deserialize)]
#[serde(untagged)]
enum MulticallEntry<T> {
    Result((T,)),
    Fault(RpcErrorBody),
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
            retry_count: 0,
            retry_delay: DEFAULT_RETRY_DELAY,
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

    /// `aria2.addUri`: queues a new download of the resource reachable at every URI of
    /// `uris` and returns its GID.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyUriList`] when `uris` is empty, or any [`Error`] of the round trip.
    pub async fn add_uri(&self, uris: &[&str], options: &AddUriOptions) -> Result<String, Error> {
        if uris.is_empty() {
            return Err(Error::EmptyUriList);
        }
        let options =
            serde_json::to_value(options).expect("download options always serialize to JSON");
        self.call("aria2.addUri", vec![Value::from(uris), options])
            .await
    }

    /// `aria2.tellStatus`: the status of the download `gid`.
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] when aria2 does not know the GID, or any other [`Error`] of the round trip.
    pub async fn tell_status(&self, gid: &str) -> Result<DownloadStatus, Error> {
        self.call("aria2.tellStatus", vec![Value::from(gid)]).await
    }

    /// `aria2.tellActive`: every active download.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the RPC round trip.
    pub async fn tell_active(&self) -> Result<Vec<DownloadStatus>, Error> {
        self.call("aria2.tellActive", Vec::new()).await
    }

    /// `aria2.tellWaiting`: at most `num` waiting or paused downloads starting at `offset`
    /// (a negative offset counts from the end of the queue).
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the RPC round trip.
    pub async fn tell_waiting(&self, offset: i64, num: u32) -> Result<Vec<DownloadStatus>, Error> {
        self.call(
            "aria2.tellWaiting",
            vec![Value::from(offset), Value::from(num)],
        )
        .await
    }

    /// `aria2.tellStopped`: at most `num` stopped downloads starting at `offset`
    /// (a negative offset counts from the most recently stopped one).
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the RPC round trip.
    pub async fn tell_stopped(&self, offset: i64, num: u32) -> Result<Vec<DownloadStatus>, Error> {
        self.call(
            "aria2.tellStopped",
            vec![Value::from(offset), Value::from(num)],
        )
        .await
    }

    /// `aria2.forceRemove`: stops the download `gid` immediately and returns its GID.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the RPC round trip.
    pub async fn force_remove(&self, gid: &str) -> Result<String, Error> {
        self.call("aria2.forceRemove", vec![Value::from(gid)]).await
    }

    /// `aria2.removeDownloadResult`: forgets the stopped download `gid`.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the RPC round trip.
    pub async fn remove_download_result(&self, gid: &str) -> Result<(), Error> {
        let _: IgnoredAny = self
            .call("aria2.removeDownloadResult", vec![Value::from(gid)])
            .await?;
        Ok(())
    }

    /// `aria2.pause`: pauses the download `gid` and returns its GID.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the RPC round trip.
    pub async fn pause(&self, gid: &str) -> Result<String, Error> {
        self.call("aria2.pause", vec![Value::from(gid)]).await
    }

    /// `aria2.unpause`: resumes the paused download `gid` and returns its GID.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the RPC round trip.
    pub async fn unpause(&self, gid: &str) -> Result<String, Error> {
        self.call("aria2.unpause", vec![Value::from(gid)]).await
    }

    /// Every download aria2 knows about, fetched with one `system.multicall` request:
    /// up to [`TELL_ALL_LIMIT`] stopped downloads, then up to [`TELL_ALL_LIMIT`] waiting
    /// downloads, then all active downloads.
    ///
    /// # Errors
    ///
    /// [`Error::Rpc`] when aria2 rejects the batch or any call in it, or any other [`Error`]
    /// of the round trip.
    pub async fn tell_all(&self) -> Result<Vec<DownloadStatus>, Error> {
        let window = || vec![Value::from(0), Value::from(TELL_ALL_LIMIT)];
        let lists: Vec<Vec<DownloadStatus>> = self
            .multicall([
                ("aria2.tellStopped", window()),
                ("aria2.tellWaiting", window()),
                ("aria2.tellActive", Vec::new()),
            ])
            .await?;
        Ok(lists.into_iter().flatten().collect())
    }

    /// Calls `method` with the secret token (when configured) followed by `args`.
    async fn call<T: DeserializeOwned>(&self, method: &str, args: Vec<Value>) -> Result<T, Error> {
        self.invoke(method, &self.with_token(args)).await
    }

    /// Runs several calls in one `system.multicall` request. The token goes into each call's
    /// parameters, never into the batch itself.
    async fn multicall<'a, T: DeserializeOwned>(
        &self,
        calls: impl IntoIterator<Item = (&'a str, Vec<Value>)>,
    ) -> Result<Vec<T>, Error> {
        let calls: Vec<Value> = calls
            .into_iter()
            .map(|(method, args)| json!({"methodName": method, "params": self.with_token(args)}))
            .collect();
        let entries: Vec<MulticallEntry<T>> = self
            .invoke("system.multicall", &[Value::Array(calls)])
            .await?;
        entries
            .into_iter()
            .map(|entry| match entry {
                MulticallEntry::Result((value,)) => Ok(value),
                MulticallEntry::Fault(fault) => Err(fault.into()),
            })
            .collect()
    }

    /// Calls `method` with exactly `params`.
    async fn invoke<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &[Value],
    ) -> Result<T, Error> {
        let body = self.post(method, params).await?;
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
        let (status, body) = self.send_with_retries(&request).await?;
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

    async fn send_with_retries(
        &self,
        request: &Request<'_>,
    ) -> Result<(StatusCode, String), Error> {
        let mut retry: u32 = 0;
        loop {
            match self.send(request).await {
                Ok(response) => return Ok(response),
                Err(error) if retry < self.retry_count => {
                    retry += 1;
                    tracing::debug!(
                        method = request.method,
                        retry,
                        error = %error,
                        "aria2 request failed, retrying"
                    );
                    tokio::time::sleep(self.retry_delay * retry).await;
                }
                Err(error) => return Err(Error::Transport(error)),
            }
        }
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

//! Client for the HIGHWAY cloud download center (`high-way.me`).
//!
//! The whole API is one endpoint, `transfers.php`, selected by query
//! parameters. Only torrent transfers are managed.

use std::fmt;
use std::time::Duration;

use jiff::Timestamp;
use rdc_model::{RemoteFile, Torrent};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::error::DebridError;
use crate::http::{DEFAULT_RATE_LIMIT_COOLDOWN, ProviderHttp};
use crate::provider::{DebridUser, ProviderConfig, RemoteTorrent};

/// The public HIGHWAY API endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://cloud.high-way.me/center-api/transfers.php";

/// The only service rd-client uses.
const SERVICE: &str = "torrent";

/// Account name reported when the server has no statistics.
const FALLBACK_USERNAME: &str = "HIGHWAY";

/// Whether `id` is a spool reference: the 32-hex id of a queued transfer
/// that has no numeric transfer id yet.
pub fn is_spool_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Client for a HIGHWAY account.
#[derive(Clone)]
pub struct Highway {
    http: ProviderHttp,
    api_key: String,
    timeout: Duration,
    endpoint: String,
}

impl fmt::Debug for Highway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Highway")
            .field("api_key", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Highway {
    /// Creates a client for the account of the API token `config.api_key`;
    /// every request times out after `config.timeout`.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self {
            http,
            api_key: config.api_key.clone(),
            timeout: config.timeout,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        }
    }

    /// Sends requests to `endpoint` instead of the public API.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        endpoint.clone_into(&mut self.endpoint);
        self
    }

    /// Lists the torrent transfers of the account (other services are skipped).
    pub async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        let list: Option<TransferList> = self
            .call(Method::GET, &[])
            .await?
            .as_deref()
            .map(decode)
            .transpose()?;
        Ok(list
            .and_then(|list| list.transfers)
            .unwrap_or_default()
            .into_iter()
            .filter(|transfer| transfer.service.as_deref() != Some("usenet"))
            .map(Transfer::into_remote)
            .collect())
    }

    /// Returns the account from the server statistics.
    ///
    /// Servers without statistics (or answering with something else) yield
    /// a stub account named `HIGHWAY`.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let stats: Stats = self
            .call(Method::GET, &[("stats", "1")])
            .await?
            .and_then(|body| serde_json::from_str(&body).ok())
            .unwrap_or_default();
        let username = non_blank(stats.username.as_deref())
            .or_else(|| non_blank(stats.plan.as_deref()))
            .unwrap_or(FALLBACK_USERNAME)
            .to_owned();
        Ok(DebridUser {
            username: Some(username),
            expiration: stats
                .plan_expires_at
                .and_then(|seconds| Timestamp::from_second(seconds).ok()),
            premium: stats.premium.unwrap_or(false),
        })
    }

    /// Adds a magnet link and returns the transfer id (or spool id).
    pub async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError> {
        self.add(Form::new().text("magnet", magnet.to_owned()))
            .await
    }

    /// Uploads a `.torrent` file and returns the transfer id (or spool id).
    pub async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError> {
        let file = Part::bytes(bytes.to_vec())
            .file_name("file.torrent")
            .mime_str("application/octet-stream")?;
        self.add(Form::new().part("file", file)).await
    }

    async fn add(&self, form: Form) -> Result<String, DebridError> {
        let request = self.request(Method::POST, &[])?.multipart(form);
        let body = self.send(request).await.map_err(rate_limit_from_text)?;
        let transfer: Option<Transfer> = body.as_deref().map(decode).transpose()?;
        transfer
            .and_then(|transfer| transfer.id)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                DebridError::InvalidResponse("HIGHWAY API did not return a transfer id.".to_owned())
            })
    }

    /// Removes the transfer; a torrent without provider id is left alone.
    pub async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError> {
        let Some(id) = non_blank(torrent.rd_id.as_deref()) else {
            return Ok(());
        };
        self.call(Method::DELETE, &id_query(id, service_for(id)))
            .await?;
        Ok(())
    }

    /// Sends a request built by [`request`](Self::request) and returns its
    /// body; `None` for 404 and empty bodies.
    async fn call(
        &self,
        method: Method,
        query: &[(&str, &str)],
    ) -> Result<Option<String>, DebridError> {
        self.send(self.request(method, query)?).await
    }

    fn request(
        &self,
        method: Method,
        query: &[(&str, &str)],
    ) -> Result<RequestBuilder, DebridError> {
        if self.api_key.trim().is_empty() {
            return Err(DebridError::NotConfigured(
                "The HIGHWAY API token is not set (provider API key)".to_owned(),
            ));
        }
        Ok(self
            .http
            .client()
            .request(method, &self.endpoint)
            .query(query)
            .bearer_auth(&self.api_key)
            .timeout(self.timeout))
    }

    /// Sends a request; `None` for 404 and empty bodies.
    async fn send(&self, request: RequestBuilder) -> Result<Option<String>, DebridError> {
        let response = self.http.send(request).await.inspect_err(|error| {
            if let DebridError::Transport(source) = error
                && source.is_timeout()
            {
                tracing::error!("the connection to HIGHWAY timed out: {source}");
            }
        })?;
        let status = response.status();
        let body = response.text().await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(error_from(status, &body));
        }
        Ok((!body.trim().is_empty()).then_some(body))
    }
}

/// The service to name for `id`: spool references are looked up without one.
fn service_for(id: &str) -> Option<&'static str> {
    (!is_spool_id(id)).then_some(SERVICE)
}

fn id_query<'a>(id: &'a str, service: Option<&'a str>) -> Vec<(&'static str, &'a str)> {
    let mut query = vec![("id", id)];
    if let Some(service) = service {
        query.push(("service", service));
    }
    query
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

/// Treats an add failure whose text asks to slow down as a two-minute rate limit.
fn rate_limit_from_text(error: DebridError) -> DebridError {
    if matches!(error, DebridError::RateLimited { .. }) {
        return error;
    }
    let message = error.to_string();
    let lowercase = message.to_lowercase();
    if lowercase.contains("slow_down") || lowercase.contains("rate limit exceeded") {
        DebridError::RateLimited {
            message,
            retry_after: DEFAULT_RATE_LIMIT_COOLDOWN,
        }
    } else {
        error
    }
}

fn decode<T: DeserializeOwned>(body: &str) -> Result<T, DebridError> {
    serde_json::from_str(body).map_err(|error| {
        DebridError::InvalidResponse(format!("HIGHWAY returned an unreadable response: {error}"))
    })
}

/// Error body of a failed request.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ApiError {
    error: Option<String>,
    message: Option<String>,
}

fn error_from(status: StatusCode, body: &str) -> DebridError {
    let ApiError { error, message } = serde_json::from_str(body).unwrap_or_default();
    let code = error.unwrap_or_default();
    let with_message = |text: String| match message.as_deref() {
        Some(message) if !message.trim().is_empty() => format!("{text} {message}"),
        _ => text,
    };
    match code.as_str() {
        "invalid_token" | "authentication_required" => {
            DebridError::NotConfigured(with_message(format!(
                "HIGHWAY authentication failed ({code}). Check the API token in the provider settings."
            )))
        }
        "system_feature_unavailable" => DebridError::NotConfigured(with_message(format!(
            "The HIGHWAY token does not belong to a high-way.me account ({code})."
        ))),
        _ => {
            let label = if code.is_empty() {
                status.as_u16().to_string()
            } else {
                format!("{} {code}", status.as_u16())
            };
            let detail = message.unwrap_or_else(|| body.trim().to_owned());
            DebridError::Provider(format!("HIGHWAY request failed ({label}): {detail}"))
        }
    }
}

/// Answer of `stats=1`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Stats {
    username: Option<String>,
    plan: Option<String>,
    /// Unix seconds; `None` for plans without expiry.
    plan_expires_at: Option<i64>,
    premium: Option<bool>,
}

/// Reads an id sent either as a string or as a number.
fn id_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Text(String),
        Number(i64),
    }
    Ok(Option::<Id>::deserialize(deserializer)?.map(|id| match id {
        Id::Text(text) => text,
        Id::Number(number) => number.to_string(),
    }))
}

/// Answer of the transfer listing.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TransferList {
    transfers: Option<Vec<Transfer>>,
}

/// One transfer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Transfer {
    #[serde(deserialize_with = "id_text")]
    id: Option<String>,
    /// `torrent` or `usenet`.
    service: Option<String>,
    name: Option<String>,
    /// `queued`, `downloading`, `completed`, `error`, ...
    state: Option<String>,
    state_detail: Option<String>,
    /// Percent.
    progress: i64,
    size: i64,
    downloaded: i64,
    /// Seconds left.
    eta: Option<i64>,
    seeder: i64,
    /// Location in the shared file tree, e.g. `TORRENT/<name>`.
    content_path: Option<String>,
    files: Option<Vec<TransferFile>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TransferFile {
    path: Option<String>,
    size: i64,
    partial: bool,
    /// Signed link to this file, when the server signs file URLs.
    download_url: Option<String>,
}

impl Transfer {
    /// Maps the transfer; it has no info hash, so the id stands in for it.
    /// `added` is now, `ended` is now once completed.
    fn into_remote(self) -> RemoteTorrent {
        let now = Timestamp::now();
        let speed = self
            .eta
            .filter(|&eta| eta > 0 && self.size > self.downloaded && self.downloaded >= 0)
            .map(|eta| (self.size - self.downloaded) / eta);
        let files = self
            .files
            .unwrap_or_default()
            .into_iter()
            .zip(0_i64..)
            .map(|(file, id)| RemoteFile {
                id,
                path: file.path.unwrap_or_default(),
                bytes: file.size,
                selected: !file.partial,
                download_link: file.download_url,
            })
            .collect();
        let id = self.id.unwrap_or_default();
        RemoteTorrent {
            hash: id.clone(),
            id,
            filename: self.name.clone().unwrap_or_default(),
            original_filename: self.name,
            bytes: self.size,
            original_bytes: self.size,
            host: self.content_path,
            progress: self.progress,
            ended: (self.state.as_deref() == Some("completed")).then_some(now),
            status: self.state,
            message: self.state_detail,
            added: Some(now),
            files: Some(files),
            links: Some(Vec::new()),
            speed,
            seeders: Some(self.seeder),
            ..RemoteTorrent::default()
        }
    }
}

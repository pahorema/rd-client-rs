//! TorBox client.
//!
//! Calls authenticate with a bearer token. Every response, successful or
//! not, is an envelope `{success, error, detail, data}`: a set `error` or
//! `success: false` is a failure whose `error` is a machine-readable code.

use std::fmt;
use std::time::Duration;

use jiff::Timestamp;
use rdc_model::RemoteFile;
use reqwest::{Method, RequestBuilder, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::error::DebridError;
use crate::http::{DEFAULT_RATE_LIMIT_COOLDOWN, ProviderHttp, RetryPolicy};
use crate::provider::{DebridUser, ProviderConfig, RemoteTorrent};

/// Address of the TorBox API.
pub const DEFAULT_BASE_URL: &str = "https://api.torbox.app/v1/api/";

/// Most torrents (and, separately, queued torrents) one listing returns.
const LIST_LIMIT: &str = "1000";

/// Download state of torrents TorBox has queued but not started yet.
const QUEUED_STATE: &str = "queued";

/// Error codes TorBox reports for failures that usually go away on retry.
const TRANSIENT_ERRORS: [&str; 4] = [
    "DATABASE_ERROR",
    "DOWNLOAD_SERVER_ERROR",
    "UNKNOWN_ERROR",
    "NO_SERVERS_AVAILABLE_ERROR",
];

/// Error codes meaning the account has to wait before adding more work.
const RATE_LIMIT_ERRORS: [&str; 2] = ["RATE_LIMIT", "ACTIVE_LIMIT"];

/// Longest part of an unexpected response body quoted in error messages.
const BODY_EXCERPT_CHARS: usize = 300;

/// Longest wait between retries of a transient TorBox error.
const MAX_ERROR_RETRY_DELAY: Duration = Duration::from_secs(30);

/// How transient TorBox error codes are retried by default: five retries
/// waiting 1, 2, 4, 8 and 16 seconds.
const DEFAULT_ERROR_RETRY: RetryPolicy = RetryPolicy {
    max_retries: 5,
    base_delay: Duration::from_secs(1),
};

/// Client for a TorBox account.
#[derive(Clone)]
pub struct TorBox {
    http: ProviderHttp,
    base_url: String,
    /// Host whose cooldown is extended when TorBox asks to slow down.
    host: String,
    api_key: String,
    timeout: Duration,
    prefer_zipped_downloads: bool,
    error_retry: RetryPolicy,
}

impl fmt::Debug for TorBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorBox")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("prefer_zipped_downloads", &self.prefer_zipped_downloads)
            .field("error_retry", &self.error_retry)
            .finish_non_exhaustive()
    }
}

impl TorBox {
    /// Creates a client for the account of `config.api_key`.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self {
            http,
            base_url: String::new(),
            host: String::new(),
            api_key: config.api_key.clone(),
            timeout: config.timeout,
            prefer_zipped_downloads: config.prefer_zipped_downloads,
            error_retry: DEFAULT_ERROR_RETRY,
        }
        .with_base_url(DEFAULT_BASE_URL)
    }

    /// Sends requests to `base_url` instead of [`DEFAULT_BASE_URL`].
    #[must_use]
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = if base_url.ends_with('/') {
            base_url.to_owned()
        } else {
            format!("{base_url}/")
        };
        self.host = Url::parse(&self.base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        self
    }

    /// Replaces how transient TorBox error codes are retried; each retry
    /// waits twice as long as the previous one, at most 30 seconds.
    #[must_use]
    pub fn with_error_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.error_retry = policy;
        self
    }

    /// Returns the account the API key belongs to.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let user = self.account(false).await?;
        let expiration = if user.plan.unwrap_or_default() == 0 {
            None
        } else {
            user.premium_expires_at
        };
        Ok(DebridUser {
            username: user.email,
            expiration,
            premium: false,
        })
    }

    async fn account(&self, with_settings: bool) -> Result<User, DebridError> {
        let settings = if with_settings { "true" } else { "false" };
        self.call(|| self.request(Method::GET, "user/me", &[("settings", settings)]))
            .await?
            .ok_or_else(|| DebridError::InvalidResponse("TorBox returned no user".to_owned()))
    }

    /// Lists the torrents of the account followed by its queued torrents
    /// (at most 1000 of each; queued torrents have their own ids).
    pub async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        let current: Option<Vec<TorrentInfo>> = self
            .call(|| {
                self.request(
                    Method::GET,
                    "torrents/mylist",
                    &[("bypass_cache", "true"), ("limit", LIST_LIMIT)],
                )
            })
            .await?;
        let queued: Option<Vec<QueuedTorrent>> = self.call(|| self.queued_request(None)).await?;
        Ok(current
            .into_iter()
            .flatten()
            .map(TorrentInfo::into_remote)
            .chain(queued.into_iter().flatten().map(QueuedTorrent::into_remote))
            .collect())
    }

    /// Looks the torrent `id` up among the current torrents, then among the
    /// queued ones; `None` when neither has it.
    pub async fn torrent(&self, id: &str) -> Result<Option<RemoteTorrent>, DebridError> {
        let id = parse_id(id)?.to_string();
        let current: Result<Option<OneOrMany<TorrentInfo>>, _> = self
            .call(|| {
                self.request(
                    Method::GET,
                    "torrents/mylist",
                    &[("id", &id), ("bypass_cache", "true"), ("limit", LIST_LIMIT)],
                )
            })
            .await;
        match current {
            Ok(found) => {
                if let Some(torrent) = found.and_then(OneOrMany::into_first) {
                    return Ok(Some(torrent.into_remote()));
                }
            }
            Err(error) if error.is_not_found() => {}
            Err(error) => return Err(error),
        }
        match self.call(|| self.queued_request(Some(&id))).await {
            Ok(found) => Ok(found
                .and_then(OneOrMany::into_first)
                .map(QueuedTorrent::into_remote)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn queued_request(&self, id: Option<&str>) -> Result<RequestBuilder, DebridError> {
        let mut params = vec![("type", "torrent"), ("bypass_cache", "true")];
        params.extend(id.map(|id| ("id", id)));
        params.extend([("offset", "0"), ("limit", LIST_LIMIT)]);
        self.request(Method::GET, "queued/getqueued", &params)
    }

    fn api_key(&self) -> Result<&str, DebridError> {
        if self.api_key.trim().is_empty() {
            return Err(DebridError::NotConfigured(
                "TorBox API key not set in the settings".to_owned(),
            ));
        }
        Ok(&self.api_key)
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<RequestBuilder, DebridError> {
        let api_key = self.api_key()?;
        Ok(self
            .http
            .client()
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(api_key)
            .query(params)
            .timeout(self.timeout))
    }

    /// Sends the request made by `build` and returns the envelope's `data`,
    /// retrying transient TorBox errors with a freshly built request.
    async fn call<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> Result<RequestBuilder, DebridError> + Send + Sync,
    ) -> Result<Option<T>, DebridError> {
        let mut attempt = 0;
        loop {
            match self.call_once(build()?).await {
                Ok(data) => return Ok(data),
                Err(Failure::Api(error))
                    if error.is_transient() && attempt < self.error_retry.max_retries =>
                {
                    let delay = self
                        .error_retry
                        .base_delay
                        .saturating_mul(2u32.saturating_pow(attempt))
                        .min(MAX_ERROR_RETRY_DELAY);
                    tracing::debug!(code = %error.code, attempt, ?delay, "retrying TorBox request");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(failure) => return Err(self.to_error(failure)),
            }
        }
    }

    async fn call_once<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> Result<Option<T>, Failure> {
        let response = self.http.send(request).await?;
        let status = response.status();
        let body = response.text().await.map_err(DebridError::from)?;
        parse_response(status, &body)
    }

    /// Turns a failure into the error reported to callers, starting a
    /// cooldown when TorBox asks to slow down.
    fn to_error(&self, failure: Failure) -> DebridError {
        let error = match failure {
            Failure::Api(error) if error.is_rate_limit() => {
                return self.rate_limited(error.message);
            }
            Failure::Api(error) if error.code.eq_ignore_ascii_case("ITEM_NOT_FOUND") => {
                DebridError::NotFound(error.message)
            }
            Failure::Api(error) => DebridError::Provider(error.message),
            Failure::Other(error) => error,
        };
        if !matches!(error, DebridError::RateLimited { .. })
            && error.to_string().to_lowercase().contains("slow_down")
        {
            return self.rate_limited(error.to_string());
        }
        error
    }

    fn rate_limited(&self, message: String) -> DebridError {
        self.http
            .coordinator()
            .update_cooldown(&self.host, DEFAULT_RATE_LIMIT_COOLDOWN);
        DebridError::RateLimited {
            message,
            retry_after: DEFAULT_RATE_LIMIT_COOLDOWN,
        }
    }
}

/// Why a single TorBox call failed.
#[derive(Debug)]
enum Failure {
    /// TorBox reported an error code.
    Api(ApiError),
    /// The call failed before TorBox could answer, or the answer was unusable.
    Other(DebridError),
}

impl From<DebridError> for Failure {
    fn from(error: DebridError) -> Self {
        Self::Other(error)
    }
}

#[derive(Debug)]
struct ApiError {
    code: String,
    message: String,
}

impl ApiError {
    fn new(code: Option<&str>, detail: Option<&str>) -> Self {
        let code = code.unwrap_or("UNKNOWN_ERROR");
        let detail = detail.filter(|detail| !detail.trim().is_empty());
        let message = match detail {
            None => code.to_owned(),
            Some(detail) if code.trim().is_empty() || detail.eq_ignore_ascii_case(code) => {
                if code.trim().is_empty() { detail } else { code }.to_owned()
            }
            Some(detail) if starts_with_ignore_case(detail, &format!("{code}:")) => {
                detail.to_owned()
            }
            Some(detail) => format!("{code}: {detail}"),
        };
        Self {
            code: code.to_owned(),
            message,
        }
    }

    fn is_transient(&self) -> bool {
        TRANSIENT_ERRORS
            .iter()
            .any(|code| self.code.eq_ignore_ascii_case(code))
    }

    fn is_rate_limit(&self) -> bool {
        RATE_LIMIT_ERRORS
            .iter()
            .any(|code| self.code.eq_ignore_ascii_case(code))
    }
}

/// The status part of a TorBox envelope.
#[derive(Debug, Deserialize)]
struct Status {
    success: Option<bool>,
    error: Option<String>,
    detail: Option<String>,
}

/// The payload part of a TorBox envelope.
#[derive(Debug, Deserialize)]
struct Data<T> {
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct User {
    email: Option<String>,
    plan: Option<i64>,
    #[serde(default, deserialize_with = "timestamp")]
    premium_expires_at: Option<Timestamp>,
}

/// A torrent of the account.
#[derive(Debug, Deserialize)]
struct TorrentInfo {
    id: i64,
    hash: Option<String>,
    name: Option<String>,
    size: Option<i64>,
    #[serde(default, deserialize_with = "timestamp")]
    created_at: Option<Timestamp>,
    #[serde(default, deserialize_with = "timestamp")]
    updated_at: Option<Timestamp>,
    download_state: Option<String>,
    seeds: Option<i64>,
    /// Completion as a fraction between 0 and 1.
    progress: Option<f64>,
    download_speed: Option<i64>,
    /// Whether the files are ready to download.
    download_present: Option<bool>,
    files: Option<Vec<TorrentFile>>,
}

#[derive(Debug, Deserialize)]
struct TorrentFile {
    id: i64,
    /// Path starting with the torrent folder, e.g. `Release/file.mkv`.
    name: Option<String>,
    size: Option<i64>,
}

impl TorrentInfo {
    /// Converts to the common form: the host is whether the download is
    /// present (`"True"`/`"False"`) and file paths drop the torrent folder.
    fn into_remote(self) -> RemoteTorrent {
        let name = self.name.unwrap_or_default();
        let size = self.size.unwrap_or_default();
        let files = self
            .files
            .unwrap_or_default()
            .into_iter()
            .map(|file| RemoteFile {
                id: file.id,
                path: file
                    .name
                    .as_deref()
                    .and_then(|name| name.split_once('/'))
                    .map(|(_, path)| path.to_owned())
                    .unwrap_or_default(),
                bytes: file.size.unwrap_or_default(),
                selected: true,
                download_link: None,
            })
            .collect();
        let present = if self.download_present.unwrap_or_default() {
            "True"
        } else {
            "False"
        };
        RemoteTorrent {
            id: self.id.to_string(),
            original_filename: Some(name.clone()),
            filename: name,
            hash: self.hash.unwrap_or_default(),
            bytes: size,
            original_bytes: size,
            host: Some(present.to_owned()),
            progress: percent(self.progress.unwrap_or_default()),
            status: self.download_state,
            added: self.created_at,
            files: Some(files),
            links: Some(Vec::new()),
            ended: self.updated_at,
            speed: Some(self.download_speed.unwrap_or_default()),
            seeders: Some(self.seeds.unwrap_or_default()),
            ..RemoteTorrent::default()
        }
    }
}

/// A torrent TorBox accepted but has not started yet.
#[derive(Debug, Deserialize)]
struct QueuedTorrent {
    id: i64,
    hash: Option<String>,
    name: Option<String>,
    #[serde(default, deserialize_with = "timestamp")]
    created_at: Option<Timestamp>,
}

impl QueuedTorrent {
    fn into_remote(self) -> RemoteTorrent {
        TorrentInfo {
            id: self.id,
            hash: self.hash,
            name: self.name,
            size: None,
            created_at: self.created_at,
            updated_at: self.created_at,
            download_state: Some(QUEUED_STATE.to_owned()),
            seeds: None,
            progress: None,
            download_speed: None,
            download_present: None,
            files: None,
        }
        .into_remote()
    }
}

/// A `data` value TorBox sends either as one object or as a list.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    fn into_first(self) -> Option<T> {
        match self {
            Self::One(item) => Some(item),
            Self::Many(items) => items.into_iter().next(),
        }
    }
}

fn parse_response<T: DeserializeOwned>(
    status: StatusCode,
    body: &str,
) -> Result<Option<T>, Failure> {
    if status == StatusCode::NO_CONTENT {
        return Ok(None);
    }
    match serde_json::from_str::<Status>(body) {
        Ok(envelope) if envelope.detail.as_deref().is_some_and(is_rate_limit_detail) => {
            return Err(Failure::Api(ApiError::new(
                Some("RATE_LIMIT"),
                envelope.detail.as_deref(),
            )));
        }
        Ok(envelope) if envelope.error.is_some() || envelope.success == Some(false) => {
            return Err(Failure::Api(ApiError::new(
                envelope.error.as_deref(),
                envelope.detail.as_deref(),
            )));
        }
        Ok(_) => {}
        Err(_) if is_rate_limit_detail(body) => {
            return Err(Failure::Api(ApiError::new(Some("RATE_LIMIT"), Some(body))));
        }
        Err(_) if status.is_success() => {}
        Err(_) => {
            return Err(Failure::Other(DebridError::Provider(format!(
                "TorBox returned HTTP {status}: {}",
                excerpt(body)
            ))));
        }
    }
    if !status.is_success() {
        return Err(Failure::Other(DebridError::Provider(format!(
            "TorBox returned HTTP {status}: {}",
            excerpt(body)
        ))));
    }
    serde_json::from_str::<Data<T>>(body)
        .map(|envelope| envelope.data)
        .map_err(|error| {
            Failure::Other(DebridError::InvalidResponse(format!(
                "Unable to read the TorBox response ({error}). Response was: {}",
                excerpt(body)
            )))
        })
}

fn is_rate_limit_detail(detail: &str) -> bool {
    let detail = detail.to_lowercase();
    detail.contains("rate limit") || detail.contains("per 1 hour")
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

/// Reads an optional date; TorBox sends RFC 3339 instants, dates without an
/// offset are taken as UTC and unreadable dates as absent.
fn timestamp<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Timestamp>, D::Error> {
    let text: Option<String> = Option::deserialize(deserializer)?;
    Ok(text.as_deref().and_then(|text| {
        text.parse::<Timestamp>().ok().or_else(|| {
            text.parse::<jiff::civil::DateTime>()
                .ok()
                .and_then(|date| date.to_zoned(jiff::tz::TimeZone::UTC).ok())
                .map(|date| date.timestamp())
        })
    }))
}

fn excerpt(body: &str) -> &str {
    body.char_indices()
        .nth(BODY_EXCERPT_CHARS)
        .map_or(body, |(end, _)| &body[..end])
}

/// Reads a TorBox torrent id (a number).
fn parse_id(id: &str) -> Result<i64, DebridError> {
    id.trim()
        .parse()
        .map_err(|_| DebridError::Provider(format!("Invalid TorBox torrent id: {id}")))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction of 1 times 100 always fits; NaN and overflow saturate"
)]
fn percent(fraction: f64) -> i64 {
    (fraction * 100.0) as i64
}

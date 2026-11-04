//! TorBox client.
//!
//! Calls authenticate with a bearer token. Every response, successful or
//! not, is an envelope `{success, error, detail, data}`: a set `error` or
//! `success: false` is a failure whose `error` is a machine-readable code.

use std::fmt;
use std::time::Duration;

use jiff::Timestamp;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::provider::{DebridUser, ProviderConfig};

/// Address of the TorBox API.
pub const DEFAULT_BASE_URL: &str = "https://api.torbox.app/v1/api/";

/// Longest part of an unexpected response body quoted in error messages.
const BODY_EXCERPT_CHARS: usize = 300;

/// Client for a TorBox account.
#[derive(Clone)]
pub struct TorBox {
    http: ProviderHttp,
    base_url: String,
    api_key: String,
    timeout: Duration,
    prefer_zipped_downloads: bool,
}

impl fmt::Debug for TorBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorBox")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("prefer_zipped_downloads", &self.prefer_zipped_downloads)
            .finish_non_exhaustive()
    }
}

impl TorBox {
    /// Creates a client for the account of `config.api_key`.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self {
            http,
            base_url: String::new(),
            api_key: config.api_key.clone(),
            timeout: config.timeout,
            prefer_zipped_downloads: config.prefer_zipped_downloads,
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

    /// Sends the request made by `build` and returns the envelope's `data`.
    async fn call<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> Result<RequestBuilder, DebridError> + Send + Sync,
    ) -> Result<Option<T>, DebridError> {
        self.call_once(build()?).await.map_err(Self::to_error)
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

    /// Turns a failure into the error reported to callers.
    fn to_error(failure: Failure) -> DebridError {
        match failure {
            Failure::Api(error) if error.code.eq_ignore_ascii_case("ITEM_NOT_FOUND") => {
                DebridError::NotFound(error.message)
            }
            Failure::Api(error) => DebridError::Provider(error.message),
            Failure::Other(error) => error,
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

fn parse_response<T: DeserializeOwned>(
    status: StatusCode,
    body: &str,
) -> Result<Option<T>, Failure> {
    if status == StatusCode::NO_CONTENT {
        return Ok(None);
    }
    match serde_json::from_str::<Status>(body) {
        Ok(envelope) if envelope.error.is_some() || envelope.success == Some(false) => {
            return Err(Failure::Api(ApiError::new(
                envelope.error.as_deref(),
                envelope.detail.as_deref(),
            )));
        }
        Ok(_) => {}
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

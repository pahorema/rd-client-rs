//! Premiumize.me client.
//!
//! Every call authenticates with the `apikey` query parameter. Responses are
//! JSON objects whose `status` is `"success"` unless the call failed, in which
//! case `message` explains why.

use std::fmt;
use std::time::Duration;

use jiff::Timestamp;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::provider::{DebridUser, ProviderConfig};

/// Address of the Premiumize API.
pub const DEFAULT_BASE_URL: &str = "https://www.premiumize.me/api/";

/// Longest part of an unexpected response body quoted in error messages.
const BODY_EXCERPT_CHARS: usize = 300;

/// Client for a Premiumize.me account.
#[derive(Clone)]
pub struct Premiumize {
    http: ProviderHttp,
    base_url: String,
    api_key: String,
    timeout: Duration,
}

impl fmt::Debug for Premiumize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Premiumize")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Premiumize {
    /// Creates a client for the account of `config.api_key`.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self {
            http,
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key: config.api_key.clone(),
            timeout: config.timeout,
        }
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
        let account: Account = self
            .call(self.request(Method::GET, "account/info", &[])?)
            .await?;
        let expiration = account
            .premium_until
            .as_ref()
            .and_then(serde_json::Value::as_i64)
            .filter(|seconds| *seconds > 0)
            .and_then(|seconds| Timestamp::from_second(seconds).ok());
        Ok(DebridUser {
            username: Some(account.customer_id.to_string()),
            expiration,
            premium: false,
        })
    }

    fn api_key(&self) -> Result<&str, DebridError> {
        if self.api_key.trim().is_empty() {
            return Err(DebridError::NotConfigured(
                "Premiumize API key not set in the settings".to_owned(),
            ));
        }
        Ok(&self.api_key)
    }

    /// Builds a request for `path` with `params` followed by the API key.
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
            .query(params)
            .query(&[("apikey", api_key)])
            .timeout(self.timeout))
    }

    async fn call<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, DebridError> {
        let response = self.http.send(request).await?;
        let status = response.status();
        let body = response.text().await?;
        parse_response(status, &body)
    }
}

/// Fields every Premiumize response may carry.
#[derive(Debug, Deserialize)]
struct Envelope {
    status: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Account {
    customer_id: IdValue,
    /// A unix time, or `false`/`null` for accounts without premium.
    premium_until: Option<serde_json::Value>,
}

/// An identifier Premiumize sends either as a number or as a string.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IdValue {
    Number(i64),
    Text(String),
}

impl fmt::Display for IdValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(number) => write!(f, "{number}"),
            Self::Text(text) => f.write_str(text),
        }
    }
}

fn parse_response<T: DeserializeOwned>(status: StatusCode, body: &str) -> Result<T, DebridError> {
    if status == StatusCode::NO_CONTENT || body.trim().is_empty() {
        return Err(DebridError::InvalidResponse(
            "Premiumize returned no result".to_owned(),
        ));
    }
    let envelope: Envelope = serde_json::from_str(body).map_err(|error| {
        if status.is_success() {
            unreadable(body, &error)
        } else {
            DebridError::InvalidResponse(format!(
                "Premiumize returned HTTP {status}: {}",
                excerpt(body)
            ))
        }
    })?;
    if envelope.status.as_deref() != Some("success")
        && let Some(message) = envelope.message
    {
        return Err(DebridError::Provider(message));
    }
    serde_json::from_str(body).map_err(|error| unreadable(body, &error))
}

fn unreadable(body: &str, error: &serde_json::Error) -> DebridError {
    DebridError::InvalidResponse(format!(
        "Unable to read the Premiumize response ({error}). Response was: {}",
        excerpt(body)
    ))
}

fn excerpt(body: &str) -> &str {
    body.char_indices()
        .nth(BODY_EXCERPT_CHARS)
        .map_or(body, |(end, _)| &body[..end])
}

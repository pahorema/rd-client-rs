//! Client for the Debrid-Link seedbox API (`https://debrid-link.fr/api/v2/`).

use std::fmt;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::provider::{DebridUser, ProviderConfig};

/// Base URL of the public Debrid-Link API.
pub const DEFAULT_BASE_URL: &str = "https://debrid-link.fr/api/v2/";

/// Client for a Debrid-Link account.
///
/// Every response is wrapped in a `{success, value, error}` envelope; a
/// failed envelope becomes a [`DebridError::Provider`] carrying the API error code.
#[derive(Clone)]
pub struct DebridLink {
    http: ProviderHttp,
    api_key: String,
    timeout: Duration,
    base_url: String,
}

impl fmt::Debug for DebridLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DebridLink")
            .field("api_key", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl DebridLink {
    /// Creates a client for the account of `config.api_key`; every request
    /// times out after `config.timeout`.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self {
            http,
            api_key: config.api_key.clone(),
            timeout: config.timeout,
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// Sends requests to `base_url` instead of the public API.
    #[must_use]
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = if base_url.ends_with('/') {
            base_url.to_owned()
        } else {
            format!("{base_url}/")
        };
        self
    }

    /// Returns the account; the expiration is derived from the remaining
    /// premium seconds.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let infos: UserInfos = self
            .send(self.request(Method::GET, "account/infos")?)
            .await?
            .unwrap_or_default();
        let expiration = (infos.premium_left > 0)
            .then(|| {
                Timestamp::now()
                    .checked_add(SignedDuration::from_secs(infos.premium_left))
                    .ok()
            })
            .flatten();
        Ok(DebridUser {
            username: infos.username,
            expiration,
            premium: false,
        })
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, DebridError> {
        if self.api_key.trim().is_empty() {
            return Err(DebridError::NotConfigured(
                "The Debrid-Link API key is not set in the settings".to_owned(),
            ));
        }
        Ok(self
            .http
            .client()
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(&self.api_key)
            .timeout(self.timeout))
    }

    async fn send<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> Result<Option<T>, DebridError> {
        let response = self.http.send(request).await.inspect_err(|error| {
            if let DebridError::Transport(source) = error
                && source.is_timeout()
            {
                tracing::error!("the connection to Debrid-Link timed out: {source}");
            }
        })?;
        let status = response.status();
        if status == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let body = response.text().await?;
        parse_envelope(status, &body)
    }
}

/// The wrapper around every API response.
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    value: Option<T>,
    error: Option<String>,
}

fn parse_envelope<T: DeserializeOwned>(
    status: StatusCode,
    body: &str,
) -> Result<Option<T>, DebridError> {
    let envelope: Envelope<T> = match serde_json::from_str(body) {
        Ok(envelope) => envelope,
        Err(error) if status.is_success() => {
            return Err(DebridError::InvalidResponse(format!(
                "Unable to read the Debrid-Link response: {error}"
            )));
        }
        // Typically an HTML error page of a proxy or a maintenance page.
        Err(_) => return Err(http_error(status)),
    };
    if !envelope.success {
        if let Some(error) = envelope.error.filter(|error| !error.trim().is_empty()) {
            return Err(DebridError::Provider(format!("Debrid-Link error: {error}")));
        }
        if !status.is_success() {
            return Err(http_error(status));
        }
        return Err(DebridError::InvalidResponse(
            "Debrid-Link reported a failure without an error code".to_owned(),
        ));
    }
    if !status.is_success() {
        return Err(http_error(status));
    }
    Ok(envelope.value)
}

fn http_error(status: StatusCode) -> DebridError {
    DebridError::Provider(format!(
        "Debrid-Link answered with unexpected HTTP status {status}"
    ))
}

/// `account/infos` value.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct UserInfos {
    username: Option<String>,
    /// Remaining premium time in seconds.
    premium_left: i64,
}

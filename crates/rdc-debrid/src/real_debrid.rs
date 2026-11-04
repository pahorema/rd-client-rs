//! Client of the Real-Debrid REST API.

use jiff::Timestamp;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::endpoint::Endpoint;
use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::provider::{DebridUser, ProviderConfig};

/// API host used when no alternative hostname is configured.
pub const DEFAULT_API_HOSTNAME: &str = "api.real-debrid.com";

/// Error code Real-Debrid uses for an unknown torrent or link.
const RESOURCE_NOT_FOUND: i64 = 7;

/// Real-Debrid account access.
#[derive(Debug)]
pub struct RealDebrid {
    endpoint: Endpoint,
    http: ProviderHttp,
}

impl RealDebrid {
    /// Creates a client for `https://<api hostname>/rest/1.0/`; a missing or
    /// blank [`ProviderConfig::api_hostname`] selects [`DEFAULT_API_HOSTNAME`].
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        let hostname = config
            .api_hostname
            .as_deref()
            .map(str::trim)
            .filter(|hostname| !hostname.is_empty())
            .unwrap_or(DEFAULT_API_HOSTNAME);
        Self::with_base_url(config, http, &format!("https://{hostname}/rest/1.0/"))
    }

    /// Creates a client for the REST API rooted at `base_url`.
    pub fn with_base_url(config: &ProviderConfig, http: ProviderHttp, base_url: &str) -> Self {
        Self {
            endpoint: Endpoint::new("Real-Debrid", base_url, config),
            http,
        }
    }

    /// Returns the account the API key belongs to; the expiration is only
    /// reported while premium time is left.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let user: User = self.call(self.request(Method::GET, "user")?).await?;
        let expiration = if user.premium > 0 {
            user.expiration.as_deref().and_then(parse_timestamp)
        } else {
            None
        };
        Ok(DebridUser {
            username: user.username,
            expiration,
            premium: false,
        })
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, DebridError> {
        self.endpoint.request(&self.http, method, path)
    }

    /// Sends `request`; `None` when the API answered without content.
    async fn execute(&self, request: RequestBuilder) -> Result<Option<String>, DebridError> {
        let response = self.http.send(request).await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(api_error(status, &text));
        }
        Ok((status != StatusCode::NO_CONTENT).then_some(text))
    }

    /// Sends `request` and decodes its JSON answer (the default for no content).
    async fn call<T: DeserializeOwned + Default>(
        &self,
        request: RequestBuilder,
    ) -> Result<T, DebridError> {
        let Some(text) = self.execute(request).await? else {
            return Ok(T::default());
        };
        serde_json::from_str(&text).map_err(|error| {
            DebridError::InvalidResponse(format!(
                "Unable to read the Real-Debrid response ({error}). Response was: {text}"
            ))
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct User {
    username: Option<String>,
    /// Seconds of premium time left.
    #[serde(default)]
    premium: i64,
    expiration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: Option<String>,
    error_code: Option<i64>,
}

/// Maps an unsuccessful answer: a `{error, error_code}` body becomes the
/// message of its code, anything else is reported verbatim.
fn api_error(status: StatusCode, text: &str) -> DebridError {
    match serde_json::from_str::<ErrorBody>(text) {
        Ok(ErrorBody {
            error: Some(error),
            error_code,
        }) => {
            let message = error_code
                .and_then(error_message)
                .map_or(error, str::to_owned);
            if error_code == Some(RESOURCE_NOT_FOUND) {
                DebridError::NotFound(message)
            } else {
                DebridError::Provider(message)
            }
        }
        _ if text.trim().is_empty() => {
            DebridError::Provider(format!("Real-Debrid answered with HTTP status {status}"))
        }
        _ => DebridError::Provider(text.to_owned()),
    }
}

/// User-facing text of a Real-Debrid error code.
fn error_message(code: i64) -> Option<&'static str> {
    let message = match code {
        -1 => "Internal error",
        1 => "Missing parameter",
        2 => "Bad parameter value",
        3 => "Unknown method",
        4 => "Method not allowed",
        5 => "Slow down",
        6 => "Resource unreachable",
        RESOURCE_NOT_FOUND => "Resource not found",
        8 => "Bad token",
        9 => "Permission denied",
        10 => "Two-Factor authentication needed",
        11 => "Two-Factor authentication pending",
        12 => "Invalid login",
        13 => "Invalid password",
        14 => "Account locked",
        15 => "Account not activated",
        16 => "Unsupported hoster",
        17 => "Hoster in maintenance",
        18 => "Hoster limit reached",
        19 => "Hoster temporarily unavailable",
        20 => "Hoster not available for free users",
        21 => "Too many active downloads",
        22 => "IP Address not allowed",
        23 => "Traffic exhausted",
        24 => "File unavailable",
        25 => "Service unavailable",
        26 => "Upload too big",
        27 => "Upload error",
        28 => "File not allowed",
        29 => "Torrent too big",
        30 => "Torrent file invalid",
        31 => "Action already done",
        32 => "Image resolution error",
        33 => "Torrent already active",
        34 => "Too many requests",
        35 => "Infringing file",
        36 => "Fair Usage Limit",
        _ => return None,
    };
    Some(message)
}

fn parse_timestamp(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::rate_limit::RateLimitCoordinator;

    fn client(api_hostname: Option<&str>) -> RealDebrid {
        let config = ProviderConfig {
            api_key: "key".to_owned(),
            api_hostname: api_hostname.map(str::to_owned),
            timeout: Duration::from_secs(1),
            prefer_zipped_downloads: false,
        };
        let http = ProviderHttp::new(config.timeout, Arc::new(RateLimitCoordinator::new()))
            .expect("client builds");
        RealDebrid::new(&config, http)
    }

    fn user_url(client: &RealDebrid) -> String {
        client.endpoint.url("user").expect("valid url").to_string()
    }

    #[test]
    fn missing_or_blank_hostname_uses_the_default_api_host() {
        for hostname in [None, Some(""), Some("   ")] {
            assert_eq!(
                user_url(&client(hostname)),
                "https://api.real-debrid.com/rest/1.0/user",
                "hostname {hostname:?}"
            );
        }
    }

    #[test]
    fn configured_hostname_replaces_the_api_host() {
        assert_eq!(
            user_url(&client(Some("api-1.real-debrid.com"))),
            "https://api-1.real-debrid.com/rest/1.0/user"
        );
    }
}

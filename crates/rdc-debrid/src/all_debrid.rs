//! Client of the AllDebrid API (version 4.1).

use jiff::Timestamp;
use rdc_model::Torrent;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::json;
use crate::provider::{DebridUser, ProviderConfig};

/// Root of the AllDebrid API.
pub const DEFAULT_BASE_URL: &str = "https://api.alldebrid.com/v4.1/";

/// Application name sent with every request, as AllDebrid requires.
const AGENT: &str = "rd-client";

/// Error code of an unknown magnet.
const MAGNET_INVALID_ID: &str = "MAGNET_INVALID_ID";

/// AllDebrid account access.
#[derive(Debug)]
pub struct AllDebrid {
    endpoint: Endpoint,
    http: ProviderHttp,
}

impl AllDebrid {
    /// Creates a client for the public AllDebrid API.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self::with_base_url(config, http, DEFAULT_BASE_URL)
    }

    /// Creates a client for the API rooted at `base_url`.
    pub fn with_base_url(config: &ProviderConfig, http: ProviderHttp, base_url: &str) -> Self {
        Self {
            endpoint: Endpoint::new("AllDebrid", base_url, config),
            http,
        }
    }

    /// Returns the account the API key belongs to; the expiration is only
    /// reported for premium accounts.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let data: UserData = self.call(self.request(Method::GET, "user")?).await?;
        let user = data.user.ok_or_else(|| {
            DebridError::InvalidResponse("AllDebrid did not return the user".to_owned())
        })?;
        Ok(DebridUser {
            username: user.username,
            expiration: epoch(user.premium_until.filter(|seconds| *seconds > 0)),
            premium: false,
        })
    }

    /// Removes the magnet from the account; torrents without provider id
    /// need no request.
    pub async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError> {
        let Some(id) = torrent.rd_id.as_deref() else {
            return Ok(());
        };
        let request = self
            .request(Method::GET, "magnet/delete")?
            .query(&[("id", id)]);
        self.call::<IgnoredAny>(request).await?;
        Ok(())
    }

    /// Turns a restricted link into a direct download URL.
    pub async fn unrestrict(&self, link: &str) -> Result<String, DebridError> {
        let request = self
            .request(Method::GET, "link/unlock")?
            .query(&[("link", link)]);
        let unlocked: Unlocked = self.call(request).await?;
        unlocked
            .link
            .filter(|link| !link.trim().is_empty())
            .ok_or_else(|| {
                DebridError::InvalidResponse("AllDebrid did not return a download link".to_owned())
            })
    }

    /// An authenticated request carrying the application name.
    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, DebridError> {
        Ok(self
            .endpoint
            .request(&self.http, method, path)?
            .query(&[("agent", AGENT)]))
    }

    /// Sends `request` and decodes the `data` of its `{status, data, error}`
    /// envelope (the default when there is none).
    async fn call<T: DeserializeOwned + Default>(
        &self,
        request: RequestBuilder,
    ) -> Result<T, DebridError> {
        let response = self.http.send(request).await?;
        let status = response.status();
        let text = response.text().await?;
        if status == StatusCode::NO_CONTENT || (status.is_success() && text.trim().is_empty()) {
            return Ok(T::default());
        }
        let envelope: Envelope = serde_json::from_str(&text).map_err(|_| {
            if status.is_success() {
                DebridError::InvalidResponse(format!(
                    "Unable to read the AllDebrid response. Response was: {text}"
                ))
            } else {
                DebridError::Provider(format!(
                    "AllDebrid answered with HTTP status {status}. Response was: {text}"
                ))
            }
        })?;
        if envelope.status.as_deref() != Some("success") {
            return Err(match envelope.error {
                Some(error) if error.has_message() => error.into_error(),
                _ => DebridError::Provider(format!("Unknown error. Response was: {text}")),
            });
        }
        match envelope.data {
            None | Some(Value::Null) => Ok(T::default()),
            Some(data) => serde_json::from_value(data).map_err(|error| {
                DebridError::InvalidResponse(format!(
                    "Unable to read the AllDebrid response ({error}). Response was: {text}"
                ))
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Envelope {
    status: Option<String>,
    data: Option<Value>,
    error: Option<ApiError>,
}

/// An error reported by AllDebrid, for the whole call or for one item.
#[derive(Debug, Default, Deserialize)]
struct ApiError {
    code: Option<Value>,
    message: Option<String>,
}

impl ApiError {
    fn has_message(&self) -> bool {
        self.message
            .as_deref()
            .is_some_and(|message| !message.trim().is_empty())
    }

    /// The error as `"<message> (<code>)"`; unknown magnets are
    /// [`DebridError::NotFound`].
    fn into_error(self) -> DebridError {
        let code = match self.code {
            Some(Value::String(code)) => code,
            Some(Value::Null) | None => String::new(),
            Some(code) => code.to_string(),
        };
        let message = format!("{} ({code})", self.message.unwrap_or_default());
        if code == MAGNET_INVALID_ID {
            DebridError::NotFound(message)
        } else {
            DebridError::Provider(message)
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct UserData {
    user: Option<User>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
    username: Option<String>,
    /// End of the premium time in Unix seconds, 0 for free accounts.
    #[serde(default, deserialize_with = "json::optional_i64")]
    premium_until: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct Unlocked {
    link: Option<String>,
}

/// The instant `seconds` after the Unix epoch.
fn epoch(seconds: Option<i64>) -> Option<Timestamp> {
    Timestamp::from_second(seconds?).ok()
}

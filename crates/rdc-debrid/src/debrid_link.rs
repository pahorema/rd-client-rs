//! Client for the Debrid-Link seedbox API (`https://debrid-link.fr/api/v2/`).

use std::fmt;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use rdc_model::RemoteFile;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::provider::{DebridUser, ProviderConfig, RemoteTorrent};

/// Base URL of the public Debrid-Link API.
pub const DEFAULT_BASE_URL: &str = "https://debrid-link.fr/api/v2/";

/// Torrents requested per listing page (the API maximum).
const PAGE_SIZE: &str = "50";

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

    /// Lists every torrent of the seedbox, page by page until an empty page.
    pub async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        let mut torrents = Vec::new();
        let mut page: u32 = 0;
        loop {
            let page_text = page.to_string();
            let request = self
                .request(Method::GET, "seedbox/list")?
                .query(&[("page", page_text.as_str()), ("perPage", PAGE_SIZE)]);
            let items: Vec<SeedboxTorrent> = self.send(request).await?.unwrap_or_default();
            if items.is_empty() {
                return Ok(torrents);
            }
            torrents.extend(items.into_iter().map(SeedboxTorrent::into_remote));
            page += 1;
        }
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

/// A torrent of `seedbox/list`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SeedboxTorrent {
    id: Option<String>,
    name: Option<String>,
    hash_string: Option<String>,
    server_id: Option<String>,
    peers_connected: i64,
    /// 0 stopped, 1 queued for verification, 2 verifying, 3 queued for
    /// download, 4 downloading, 5 queued for seeding, 6 seeding, 100 stored.
    status: i64,
    total_size: i64,
    files: Option<Vec<SeedboxFile>>,
    /// Unix seconds.
    created: i64,
    download_percent: i64,
    /// Bytes per second.
    download_speed: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SeedboxFile {
    name: Option<String>,
    size: i64,
    download_url: Option<String>,
}

impl SeedboxTorrent {
    fn into_remote(self) -> RemoteTorrent {
        let name = self.name.unwrap_or_default();
        let links = self.files.as_ref().map(|files| {
            files
                .iter()
                .filter_map(|file| file.download_url.clone())
                .collect()
        });
        let files = self
            .files
            .unwrap_or_default()
            .into_iter()
            .zip(0_i64..)
            .map(|(file, id)| RemoteFile {
                id,
                path: file.name.unwrap_or_default(),
                bytes: file.size,
                selected: true,
                download_link: file.download_url,
            })
            .collect();
        RemoteTorrent {
            id: self.id.unwrap_or_default(),
            original_filename: Some(name.clone()),
            filename: name,
            hash: self.hash_string.unwrap_or_default(),
            bytes: self.total_size,
            host: Some(self.server_id.unwrap_or_default()),
            progress: self.download_percent,
            status: Some(self.status.to_string()),
            status_code: self.status,
            added: Timestamp::from_second(self.created).ok(),
            files: Some(files),
            links,
            speed: Some(self.download_speed),
            seeders: Some(self.peers_connected),
            ..RemoteTorrent::default()
        }
    }
}

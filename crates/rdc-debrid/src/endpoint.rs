//! Base address, API key and timeout of a provider API authenticated with a
//! bearer token.

use std::fmt;
use std::time::Duration;

use reqwest::{Method, RequestBuilder, Url};

use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::provider::ProviderConfig;

/// Where and how to reach one provider API.
pub(crate) struct Endpoint {
    /// Provider name used in messages.
    provider: &'static str,
    /// API root (always ending with `/`), or why it is unusable.
    base_url: Result<Url, String>,
    api_key: String,
    timeout: Duration,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Endpoint {
    /// Describes the API of `provider` rooted at `base_url`.
    pub(crate) fn new(provider: &'static str, base_url: &str, config: &ProviderConfig) -> Self {
        let base_url = Url::parse(base_url)
            .map(|mut url| {
                if !url.path().ends_with('/') {
                    let path = format!("{}/", url.path());
                    url.set_path(&path);
                }
                url
            })
            .map_err(|error| format!("The {provider} API address {base_url} is invalid: {error}"));
        Self {
            provider,
            base_url,
            api_key: config.api_key.trim().to_owned(),
            timeout: config.timeout,
        }
    }

    /// Absolute URL of `path` below the API root.
    pub(crate) fn url(&self, path: &str) -> Result<Url, DebridError> {
        let base = self
            .base_url
            .as_ref()
            .map_err(|message| DebridError::NotConfigured(message.clone()))?;
        base.join(path).map_err(|error| {
            DebridError::NotConfigured(format!(
                "The {} API address {base}{path} is invalid: {error}",
                self.provider
            ))
        })
    }

    /// A request without credentials, for public endpoints.
    pub(crate) fn public_request(
        &self,
        http: &ProviderHttp,
        method: Method,
        path: &str,
    ) -> Result<RequestBuilder, DebridError> {
        Ok(http
            .client()
            .request(method, self.url(path)?)
            .timeout(self.timeout))
    }

    /// A request carrying the API key as bearer token.
    ///
    /// Fails with [`DebridError::NotConfigured`] when no API key is set.
    pub(crate) fn request(
        &self,
        http: &ProviderHttp,
        method: Method,
        path: &str,
    ) -> Result<RequestBuilder, DebridError> {
        if self.api_key.is_empty() {
            return Err(DebridError::NotConfigured(format!(
                "The {} API key is not set in the settings",
                self.provider
            )));
        }
        Ok(self
            .public_request(http, method, path)?
            .bearer_auth(&self.api_key))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::rate_limit::RateLimitCoordinator;

    fn config(api_key: &str) -> ProviderConfig {
        ProviderConfig {
            api_key: api_key.to_owned(),
            api_hostname: None,
            timeout: Duration::from_secs(1),
            prefer_zipped_downloads: false,
        }
    }

    #[test]
    fn paths_are_joined_below_the_base_path() {
        let endpoint = Endpoint::new("Test", "https://host/api/1.0", &config("key"));

        let url = endpoint.url("torrents/info/AB").expect("valid url");

        assert_eq!(url.as_str(), "https://host/api/1.0/torrents/info/AB");
    }

    #[test]
    fn an_invalid_base_address_is_reported_as_not_configured() {
        let endpoint = Endpoint::new("Test", "https://bad host/", &config("key"));

        let error = endpoint.url("user").expect_err("no host");

        assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
        assert!(error.to_string().contains("Test API address"), "{error}");
    }

    #[test]
    fn a_blank_api_key_is_reported_as_not_configured() {
        let endpoint = Endpoint::new("Test", "https://host/", &config("  "));
        let http = ProviderHttp::new(
            Duration::from_secs(1),
            Arc::new(RateLimitCoordinator::new()),
        )
        .expect("client builds");

        let error = endpoint
            .request(&http, Method::GET, "user")
            .expect_err("no key");

        assert_eq!(
            error.to_string(),
            "The Test API key is not set in the settings"
        );
    }
}

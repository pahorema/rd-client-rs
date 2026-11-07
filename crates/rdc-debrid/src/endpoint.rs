//! Base address and API key of a provider API.

use std::fmt;

use reqwest::{Method, RequestBuilder, Url};

use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::provider::ProviderConfig;

/// Where and how to reach one provider API.
#[derive(Clone)]
pub(crate) struct Endpoint {
    /// Provider name used in messages.
    provider: &'static str,
    /// API root, or why it is unusable.
    base_url: Result<Url, String>,
    /// The API key of the settings without surrounding whitespace.
    api_key: String,
    credential: Credential,
}

/// How authenticated requests carry the API key.
#[derive(Debug, Clone, Copy)]
enum Credential {
    /// As bearer token.
    Bearer,
    /// As the query parameter with this name.
    Query(&'static str),
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("credential", &self.credential)
            .finish_non_exhaustive()
    }
}

impl Endpoint {
    /// Describes the API of `provider` whose paths are relative to the
    /// directory `base_url`; requests carry the API key as bearer token.
    pub(crate) fn new(provider: &'static str, base_url: &str, config: &ProviderConfig) -> Self {
        Self::with_root(provider, parse(provider, base_url, true), config)
    }

    /// Describes an API that is the single resource at `url` (requested
    /// with the empty path); requests carry the API key as bearer token.
    pub(crate) fn resource(provider: &'static str, url: &str, config: &ProviderConfig) -> Self {
        Self::with_root(provider, parse(provider, url, false), config)
    }

    fn with_root(
        provider: &'static str,
        base_url: Result<Url, String>,
        config: &ProviderConfig,
    ) -> Self {
        Self {
            provider,
            base_url,
            api_key: config.api_key.trim().to_owned(),
            credential: Credential::Bearer,
        }
    }

    /// Sends the API key as query parameter `name` instead of as bearer token.
    #[must_use]
    pub(crate) fn with_query_key(mut self, name: &'static str) -> Self {
        self.credential = Credential::Query(name);
        self
    }

    /// Moves the API root to the directory `base_url`.
    pub(crate) fn set_base_url(&mut self, base_url: &str) {
        self.base_url = parse(self.provider, base_url, true);
    }

    /// Moves a [`resource`](Self::resource) API to `url`.
    pub(crate) fn set_resource_url(&mut self, url: &str) {
        self.base_url = parse(self.provider, url, false);
    }

    /// Host of the API, `None` while its address is invalid.
    pub(crate) fn host(&self) -> Option<&str> {
        self.base_url.as_ref().ok()?.host_str()
    }

    /// The API key, for calls that also need it inside their parameters.
    ///
    /// Fails with [`DebridError::NotConfigured`] when no API key is set.
    pub(crate) fn api_key(&self) -> Result<&str, DebridError> {
        if self.api_key.is_empty() {
            return Err(DebridError::NotConfigured(format!(
                "The {} API key is not set in the settings",
                self.provider
            )));
        }
        Ok(&self.api_key)
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
        Ok(http.client().request(method, self.url(path)?))
    }

    /// A request carrying the API key.
    ///
    /// Fails with [`DebridError::NotConfigured`] when no API key is set.
    pub(crate) fn request(
        &self,
        http: &ProviderHttp,
        method: Method,
        path: &str,
    ) -> Result<RequestBuilder, DebridError> {
        let api_key = self.api_key()?;
        let request = self.public_request(http, method, path)?;
        Ok(match self.credential {
            Credential::Bearer => request.bearer_auth(api_key),
            Credential::Query(name) => request.query(&[(name, api_key)]),
        })
    }
}

/// Parses the API address `url` of `provider`; a `directory` address gets
/// a trailing `/` so paths are joined below it.
fn parse(provider: &str, url: &str, directory: bool) -> Result<Url, String> {
    let mut parsed = Url::parse(url)
        .map_err(|error| format!("The {provider} API address {url} is invalid: {error}"))?;
    if directory && !parsed.path().ends_with('/') {
        let path = format!("{}/", parsed.path());
        parsed.set_path(&path);
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

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

    fn http() -> ProviderHttp {
        ProviderHttp::new(
            Duration::from_secs(1),
            Arc::new(RateLimitCoordinator::new()),
        )
        .expect("client builds")
    }

    #[test]
    fn paths_are_joined_below_the_base_path() {
        let endpoint = Endpoint::new("Test", "https://host/api/1.0", &config("key"));

        let url = endpoint.url("torrents/info/AB").expect("valid url");

        assert_eq!(url.as_str(), "https://host/api/1.0/torrents/info/AB");
    }

    #[test]
    fn resource_addresses_are_used_as_they_are() {
        let endpoint = Endpoint::resource("Test", "https://host/api/calls.php", &config("key"));

        let url = endpoint.url("").expect("valid url");

        assert_eq!(url.as_str(), "https://host/api/calls.php");
    }

    #[test]
    fn query_keys_are_sent_as_query_parameters() {
        let endpoint =
            Endpoint::new("Test", "https://host/api/", &config(" key\n")).with_query_key("apikey");

        let request = endpoint
            .request(&http(), Method::GET, "user")
            .expect("configured")
            .build()
            .expect("valid request");

        assert_eq!(request.url().as_str(), "https://host/api/user?apikey=key");
        assert!(!request.headers().contains_key("authorization"));
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
        let http = http();

        let error = endpoint
            .request(&http, Method::GET, "user")
            .expect_err("no key");

        assert_eq!(
            error.to_string(),
            "The Test API key is not set in the settings"
        );
    }
}

//! Shared state of one running download and the HTTP requests it sends.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CACHE_CONTROL};
use reqwest::{Client, Response, Url, redirect};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::config::InternalConfig;
use super::error::TransferError;

/// `User-Agent` of every request.
const USER_AGENT: &str = concat!("rd-client/", env!("CARGO_PKG_VERSION"));
/// Redirects followed before a request fails.
const MAX_REDIRECTS: usize = 50;

/// State shared by every request of one download.
#[derive(Debug)]
pub(crate) struct Context {
    pub(crate) config: watch::Receiver<InternalConfig>,
    pub(crate) cancel: CancellationToken,
    received: AtomicU64,
    client: Mutex<Option<Client>>,
}

impl Context {
    pub(crate) fn new(config: watch::Receiver<InternalConfig>, cancel: CancellationToken) -> Self {
        Self {
            config,
            cancel,
            received: AtomicU64::new(0),
            client: Mutex::new(None),
        }
    }

    /// A snapshot of the current configuration.
    pub(crate) fn config(&self) -> InternalConfig {
        self.config.borrow().clone()
    }

    /// Bytes written to the file so far.
    pub(crate) fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    pub(crate) fn add_received(&self, bytes: u64) {
        self.received.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Runs `future` unless `cancel` fires first.
    pub(crate) async fn cancellable<T>(
        cancel: &CancellationToken,
        future: impl Future<Output = T>,
    ) -> Result<T, TransferError> {
        tokio::select! {
            () = cancel.cancelled() => Err(TransferError::Cancelled),
            value = future => Ok(value),
        }
    }

    /// Sends a GET for `url` and returns a successful response.
    pub(crate) async fn get(
        &self,
        url: &Url,
        cancel: &CancellationToken,
    ) -> Result<Response, TransferError> {
        let request = self
            .client()?
            .get(url.clone())
            .header(ACCEPT, "*/*")
            .header(ACCEPT_ENCODING, "identity")
            .header(CACHE_CONTROL, "no-cache");
        let limit = self.config.borrow().header_timeout();
        let response =
            match Self::cancellable(cancel, tokio::time::timeout(limit, request.send())).await? {
                Ok(result) => result.map_err(TransferError::Http)?,
                Err(_) => return Err(TransferError::HeaderTimeout(limit)),
            };
        if !response.status().is_success() {
            return Err(TransferError::Status(response.status()));
        }
        Ok(response)
    }

    /// Returns the download's client, building it on first use.
    fn client(&self) -> Result<Client, TransferError> {
        let mut cached = self.client.lock().expect("client cache lock poisoned");
        if let Some(client) = cached.as_ref() {
            return Ok(client.clone());
        }
        let client = build_client()?;
        *cached = Some(client.clone());
        Ok(client)
    }
}

fn build_client() -> Result<Client, TransferError> {
    Client::builder()
        .user_agent(USER_AGENT)
        .http1_only()
        .redirect(redirect::Policy::limited(MAX_REDIRECTS))
        .no_proxy()
        .build()
        .map_err(TransferError::Http)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_the_product_and_version() {
        assert_eq!(
            USER_AGENT,
            format!("rd-client/{}", env!("CARGO_PKG_VERSION"))
        );
    }
}

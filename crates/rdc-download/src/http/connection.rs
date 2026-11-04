//! Shared state of one running download and the HTTP requests it sends.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CACHE_CONTROL, RANGE};
use reqwest::{Client, Response, Url, redirect};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::config::InternalConfig;
use super::error::TransferError;
use super::rate::Throttle;

/// `User-Agent` of every request.
const USER_AGENT: &str = concat!("rd-client/", env!("CARGO_PKG_VERSION"));
/// Redirects followed before a request fails.
const MAX_REDIRECTS: usize = 50;

/// An inclusive byte range to request; `end: None` reads to the end of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ByteRange {
    pub(crate) start: u64,
    pub(crate) end: Option<u64>,
}

impl ByteRange {
    fn header_value(self) -> String {
        match self.end {
            Some(end) => format!("bytes={}-{end}", self.start),
            None => format!("bytes={}-", self.start),
        }
    }
}

/// State shared by the probe and every request of one download.
#[derive(Debug)]
pub(crate) struct Context {
    pub(crate) config: watch::Receiver<InternalConfig>,
    pub(crate) cancel: CancellationToken,
    pub(crate) throttle: Throttle,
    received: AtomicU64,
    client: Mutex<Option<Client>>,
}

impl Context {
    pub(crate) fn new(config: watch::Receiver<InternalConfig>, cancel: CancellationToken) -> Self {
        Self {
            config,
            cancel,
            throttle: Throttle::new(),
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

    pub(crate) fn reset_received(&self) {
        self.received.store(0, Ordering::Relaxed);
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

    /// Sleeps for `duration` unless `cancel` fires first.
    pub(crate) async fn sleep(
        cancel: &CancellationToken,
        duration: Duration,
    ) -> Result<(), TransferError> {
        Self::cancellable(cancel, tokio::time::sleep(duration)).await
    }

    /// Sends a GET for `url`, optionally limited to `range`, and returns a successful response.
    pub(crate) async fn get(
        &self,
        url: &Url,
        range: Option<ByteRange>,
        cancel: &CancellationToken,
    ) -> Result<Response, TransferError> {
        let mut request = self
            .client()?
            .get(url.clone())
            .header(ACCEPT, "*/*")
            .header(ACCEPT_ENCODING, "identity")
            .header(CACHE_CONTROL, "no-cache");
        if let Some(range) = range {
            request = request.header(RANGE, range.header_value());
        }
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
    fn range_header_values() {
        let closed = ByteRange {
            start: 0,
            end: Some(0),
        };
        assert_eq!(closed.header_value(), "bytes=0-0");
        let open = ByteRange {
            start: 42,
            end: None,
        };
        assert_eq!(open.header_value(), "bytes=42-");
    }

    #[test]
    fn user_agent_names_the_product_and_version() {
        assert_eq!(
            USER_AGENT,
            format!("rd-client/{}", env!("CARGO_PKG_VERSION"))
        );
    }
}

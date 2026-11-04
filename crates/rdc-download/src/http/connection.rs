//! Shared state of one running download and the HTTP requests it sends.

use std::net::IpAddr;
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
use crate::bind::resolve_bind_address;

/// `User-Agent` of every request.
const USER_AGENT: &str = concat!("rd-client/", env!("CARGO_PKG_VERSION"));
/// Redirects followed before a request fails.
const MAX_REDIRECTS: usize = 50;
/// Wait between attempts to connect through the bind address.
const BIND_RETRY_DELAY: Duration = Duration::from_secs(1);

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

/// State shared by the probe and every chunk of one download.
#[derive(Debug)]
pub(crate) struct Context {
    pub(crate) config: watch::Receiver<InternalConfig>,
    pub(crate) cancel: CancellationToken,
    pub(crate) paused: watch::Receiver<bool>,
    pub(crate) throttle: Throttle,
    received: AtomicU64,
    client: Mutex<Option<CachedClient>>,
}

#[derive(Debug)]
struct CachedClient {
    proxy: Option<String>,
    bind: Option<IpAddr>,
    client: Client,
}

impl Context {
    pub(crate) fn new(
        config: watch::Receiver<InternalConfig>,
        cancel: CancellationToken,
        paused: watch::Receiver<bool>,
    ) -> Self {
        Self {
            config,
            cancel,
            paused,
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

    /// Waits until the download is not paused.
    pub(crate) async fn wait_while_paused(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), TransferError> {
        let mut paused = self.paused.clone();
        // A closed channel means nobody can pause the download any more.
        let _ = Self::cancellable(cancel, paused.wait_for(|paused| !*paused)).await?;
        Ok(())
    }

    pub(crate) fn is_paused(&self) -> bool {
        *self.paused.borrow()
    }

    /// Sends a GET for `url`, optionally limited to `range`, and returns a successful response.
    ///
    /// With a bind address configured the request waits until the address is usable and
    /// retries connection failures every second, as long as the download is not cancelled.
    pub(crate) async fn get(
        &self,
        url: &Url,
        range: Option<ByteRange>,
        cancel: &CancellationToken,
    ) -> Result<Response, TransferError> {
        loop {
            let (client, config) = self.client(cancel).await?;
            let mut request = client
                .get(url.clone())
                .header(ACCEPT, "*/*")
                .header(ACCEPT_ENCODING, "identity")
                .header(CACHE_CONTROL, "no-cache");
            if let Some(range) = range {
                request = request.header(RANGE, range.header_value());
            }
            let sent = request.send();
            let result = match config.header_timeout() {
                Some(limit) => {
                    match Self::cancellable(cancel, tokio::time::timeout(limit, sent)).await? {
                        Ok(result) => result,
                        Err(_) => return Err(TransferError::HeaderTimeout(limit)),
                    }
                }
                None => Self::cancellable(cancel, sent).await?,
            };
            match result {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) => return Err(TransferError::Status(response.status())),
                Err(error) if config.bind_address.is_some() && error.is_connect() => {
                    tracing::warn!("Unable to connect through the bind address, retrying: {error}");
                    Self::sleep(cancel, BIND_RETRY_DELAY).await?;
                }
                Err(error) => return Err(TransferError::Http(error)),
            }
        }
    }

    /// Returns a client for the current proxy and bind settings, building it when they changed.
    async fn client(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(Client, InternalConfig), TransferError> {
        loop {
            let config = self.config();
            let bind = match config.bind_address.as_deref() {
                None => None,
                Some(configured) => {
                    if let Some(address) = resolve_bind_address(Some(configured)) {
                        Some(address)
                    } else {
                        tracing::warn!(
                            "The bind address {configured:?} is not assigned to an active network interface, retrying"
                        );
                        Self::sleep(cancel, BIND_RETRY_DELAY).await?;
                        continue;
                    }
                }
            };
            let proxy = config.proxy_url().map(str::to_owned);
            let mut cached = self.client.lock().expect("client cache lock poisoned");
            if let Some(current) = cached.as_ref()
                && current.proxy == proxy
                && current.bind == bind
            {
                return Ok((current.client.clone(), config));
            }
            let client = build_client(proxy.as_deref(), bind)?;
            *cached = Some(CachedClient {
                proxy,
                bind,
                client: client.clone(),
            });
            return Ok((client, config));
        }
    }
}

fn build_client(proxy: Option<&str>, bind: Option<IpAddr>) -> Result<Client, TransferError> {
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .http1_only()
        .redirect(redirect::Policy::limited(MAX_REDIRECTS))
        .local_address(bind);
    builder = match proxy {
        Some(proxy) => {
            let proxy_error = |error| TransferError::Proxy {
                proxy: proxy.to_owned(),
                error,
            };
            builder.proxy(reqwest::Proxy::all(proxy).map_err(proxy_error)?)
        }
        None => builder.no_proxy(),
    };
    builder.build().map_err(TransferError::Http)
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

    #[test]
    fn invalid_proxy_is_reported() {
        let error = build_client(Some("::not a url::"), None).expect_err("invalid proxy");
        assert!(
            error
                .to_string()
                .starts_with("Invalid proxy server ::not a url::"),
            "{error}"
        );
    }
}

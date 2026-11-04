//! Fetches and caches validated tracker lists for torrent enrichment.

use std::{sync::Arc, time::Duration};

use reqwest::{Client, Url};
use thiserror::Error;
use tokio::{sync::Mutex, time::Instant};
use tokio_util::sync::CancellationToken;

/// A failed tracker-list request; enrichment callers may log it and continue unchanged.
#[derive(Debug, Error)]
pub enum FetchTrackersError {
    /// The HTTP request, response status, or response body failed.
    #[error("unable to fetch tracker list for enrichment: {0}")]
    Http(#[from] reqwest::Error),
    /// The caller cancelled the request.
    #[error("fetching tracker list was cancelled")]
    Cancelled,
}

#[derive(Debug)]
struct CachedList {
    url: Url,
    expiration_minutes: i32,
    expires_at: Instant,
    trackers: Arc<[String]>,
}

/// A single-flight HTTP fetcher with a cache shared by every clone.
#[derive(Clone, Debug)]
pub struct TrackerList {
    client: Client,
    cached: Arc<Mutex<Option<CachedList>>>,
}

impl TrackerList {
    /// Creates a fetcher using a fifteen-second request timeout and the product user agent.
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(concat!("rd-client/", env!("CARGO_PKG_VERSION")))
                .build()?,
            cached: Arc::default(),
        })
    }

    /// Retrieves trackers, or an empty list for a blank or non-HTTP(S) URL.
    ///
    /// Positive expiration values are minutes. Changing the URL or expiration invalidates
    /// the cache. Nonpositive expiration disables caching; empty results are never cached.
    /// Concurrent cache misses share one request when caching is enabled.
    ///
    /// # Errors
    /// Returns HTTP/status/body failures or caller cancellation, without hiding them.
    pub async fn fetch(
        &self,
        configured_url: Option<&str>,
        expiration_minutes: i32,
        cancel: &CancellationToken,
    ) -> Result<Arc<[String]>, FetchTrackersError> {
        let url = configured_url.unwrap_or_default().trim();
        let url = Url::parse(url)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
        // Keeping the lock across the request makes cache misses single-flight, including
        // the second cache check after a preceding caller finishes fetching.
        let mut cached = tokio::select! {
            () = cancel.cancelled() => return Err(FetchTrackersError::Cancelled),
            guard = self.cached.lock() => guard,
        };
        let Some(url) = url else {
            *cached = None;
            if configured_url.is_some_and(|value| !value.trim().is_empty()) {
                tracing::warn!(url = configured_url, "Invalid tracker list URL format");
            }
            return Ok(Arc::from([]));
        };
        if let Some(entry) = cached.as_ref()
            && expiration_minutes > 0
            && entry.expiration_minutes == expiration_minutes
            && entry.url == url
            && entry.expires_at > Instant::now()
        {
            return Ok(Arc::clone(&entry.trackers));
        }
        *cached = None;
        let fetch = async {
            self.client
                .get(url.clone())
                .send()
                .await?
                .error_for_status()?
                .text()
                .await
        };
        let text = tokio::select! {
            () = cancel.cancelled() => return Err(FetchTrackersError::Cancelled),
            result = fetch => result?,
        };
        let (trackers, rejected) = rdc_torrent::parse_tracker_list(&text);
        tracing::debug!(rejected, "Trackers rejected during enrichment");
        let trackers: Arc<[String]> = trackers.into();
        if expiration_minutes > 0 && !trackers.is_empty() {
            *cached = Some(CachedList {
                url,
                expiration_minutes,
                expires_at: Instant::now()
                    + Duration::from_secs(
                        u64::try_from(expiration_minutes).unwrap_or_default() * 60,
                    ),
                trackers: Arc::clone(&trackers),
            });
        }
        Ok(trackers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    #[tokio::test]
    async fn parses_and_caches_concurrent_requests() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/list"))
            .and(header(
                "user-agent",
                concat!("rd-client/", env!("CARGO_PKG_VERSION")),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "# comment\nudp://tracker.test/announce\ninvalid\nUDP://tracker.test/announce\n",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let list = TrackerList::new().unwrap();
        let cancel = CancellationToken::new();
        let url = format!("{}/list", server.uri());
        let (first, second) = tokio::join!(
            list.fetch(Some(&url), 60, &cancel),
            list.fetch(Some(&url), 60, &cancel)
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.as_ref(), ["udp://tracker.test/announce"]);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn expiration_changes_and_disabling_invalidate_cache() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("https://tracker.test/announce"),
            )
            .expect(4)
            .mount(&server)
            .await;
        let list = TrackerList::new().unwrap();
        let cancel = CancellationToken::new();
        for expiration in [60, 60, 30, 0, 0] {
            list.fetch(Some(&server.uri()), expiration, &cancel)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn empty_results_are_not_cached() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("invalid"))
            .expect(2)
            .mount(&server)
            .await;
        let list = TrackerList::new().unwrap();
        for _ in 0..2 {
            assert!(
                list.fetch(Some(&server.uri()), 60, &CancellationToken::new())
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn failures_propagate_and_are_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .expect(2)
            .mount(&server)
            .await;
        let list = TrackerList::new().unwrap();
        for _ in 0..2 {
            assert!(matches!(
                list.fetch(Some(&server.uri()), 60, &CancellationToken::new())
                    .await,
                Err(FetchTrackersError::Http(_))
            ));
        }
    }

    #[tokio::test]
    async fn expires_entries_and_invalidates_when_url_changes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("https://tracker.test/announce"),
            )
            .expect(3)
            .mount(&server)
            .await;
        let list = TrackerList::new().unwrap();
        let cancel = CancellationToken::new();
        list.fetch(Some(&server.uri()), 1, &cancel).await.unwrap();
        list.cached.lock().await.as_mut().unwrap().expires_at =
            Instant::now() - Duration::from_secs(1);
        list.fetch(Some(&server.uri()), 1, &cancel).await.unwrap();
        list.fetch(Some(&format!("{}/other", server.uri())), 1, &cancel)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ignores_unsupported_urls_and_honours_cancellation() {
        let list = TrackerList::new().unwrap();
        for url in [
            None,
            Some(""),
            Some("ftp://tracker.test"),
            Some("not a url"),
        ] {
            assert!(
                list.fetch(url, 60, &CancellationToken::new())
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            list.fetch(Some("https://example.test"), 60, &cancel).await,
            Err(FetchTrackersError::Cancelled)
        ));
    }
}

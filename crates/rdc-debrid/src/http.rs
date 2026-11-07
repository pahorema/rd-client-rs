//! HTTP plumbing shared by all providers: user agent, timeouts, retries and
//! rate-limit handling.

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, Method, Request, RequestBuilder, Response, StatusCode};

use crate::error::DebridError;
use crate::rate_limit::RateLimitCoordinator;

/// `User-Agent` sent with every provider request.
pub const USER_AGENT: &str = concat!("rd-client/", env!("CARGO_PKG_VERSION"));

/// Cooldown applied when a provider rate-limits without saying for how long.
pub const DEFAULT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(120);

/// How transient failures (connection errors, timeouts, HTTP 408) are retried.
///
/// `POST`, `PUT` and `PATCH` requests may have been carried out by the time
/// they time out, so they are only retried when the connection failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// Delay before the first retry; doubles for every further retry (with jitter).
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay: Duration::from_secs(2),
        }
    }
}

impl RetryPolicy {
    fn delay(self, attempt: u32) -> Duration {
        let exponential = self.base_delay * 2u32.saturating_pow(attempt);
        exponential.mul_f64(0.75 + fastrand::f64() * 0.5)
    }
}

/// HTTP client used by provider implementations.
///
/// Cheap to clone; clones share the connection pool and the rate-limit coordinator.
#[derive(Debug, Clone)]
pub struct ProviderHttp {
    client: Client,
    coordinator: Arc<RateLimitCoordinator>,
    retry: RetryPolicy,
}

impl ProviderHttp {
    /// Creates a client whose requests time out after `timeout`.
    pub fn new(
        timeout: Duration,
        coordinator: Arc<RateLimitCoordinator>,
    ) -> Result<Self, DebridError> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .build()?;
        Ok(Self {
            client,
            coordinator,
            retry: RetryPolicy::default(),
        })
    }

    /// Replaces the retry policy (tests use short delays).
    #[must_use]
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The underlying client, for building requests.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The coordinator that records provider cooldowns.
    pub fn coordinator(&self) -> &Arc<RateLimitCoordinator> {
        &self.coordinator
    }

    /// Sends a request with cooldown checks, transient-failure retries and 429 handling.
    ///
    /// Any response other than 429 is returned as is; mapping provider error
    /// bodies is up to the caller. Requests with streaming bodies (multipart
    /// uploads) cannot be cloned and are sent only once; `POST`, `PUT` and
    /// `PATCH` requests are only sent again when the connection failed, as a
    /// timed-out attempt may already have added a torrent.
    ///
    /// # Errors
    ///
    /// [`DebridError::RateLimited`] when the host is cooling down or answers 429,
    /// [`DebridError::Transport`] when the request cannot be completed.
    pub async fn send(&self, request: RequestBuilder) -> Result<Response, DebridError> {
        let mut request = request.build()?;
        let host = request.url().host_str().unwrap_or("unknown").to_owned();

        let cooldown = self.coordinator.remaining_cooldown(&host);
        if !cooldown.is_zero() {
            return Err(DebridError::RateLimited {
                message: format!("Rate limit cooldown active for {host}"),
                retry_after: cooldown,
            });
        }

        let repeatable = is_repeatable(request.method());
        let mut attempt = 0;
        loop {
            let retry = self.retry_copy(&request, attempt);
            match self.client.execute(request).await {
                Ok(response) if response.status() == StatusCode::TOO_MANY_REQUESTS => {
                    return Err(self.rate_limited(&host, response.headers()));
                }
                Ok(response) if response.status() == StatusCode::REQUEST_TIMEOUT => match retry {
                    Some(next) => request = next,
                    None => return Ok(response),
                },
                Ok(response) => return Ok(response),
                Err(error) if is_transient(&error, repeatable) => match retry {
                    Some(next) => request = next,
                    None => return Err(error.into()),
                },
                Err(error) => return Err(error.into()),
            }
            let delay = self.retry.delay(attempt);
            tracing::debug!(%host, attempt, ?delay, "retrying provider request");
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }

    fn retry_copy(&self, request: &Request, attempt: u32) -> Option<Request> {
        if attempt < self.retry.max_retries {
            request.try_clone()
        } else {
            None
        }
    }

    fn rate_limited(&self, host: &str, headers: &HeaderMap) -> DebridError {
        let retry_after = retry_after_delay(headers);
        self.coordinator.update_cooldown(host, retry_after);
        DebridError::RateLimited {
            message: format!("Provider {host} rate limit exceeded"),
            retry_after,
        }
    }
}

/// Reads `Retry-After` (seconds or HTTP date); defaults to two minutes when
/// the header is missing, unparsable or in the past.
pub fn retry_after_delay(headers: &HeaderMap) -> Duration {
    let Some(value) = headers.get(RETRY_AFTER).and_then(|v| v.to_str().ok()) else {
        return DEFAULT_RATE_LIMIT_COOLDOWN;
    };
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Duration::from_secs(seconds);
    }
    jiff::fmt::rfc2822::parse(value)
        .ok()
        .and_then(|date| Duration::try_from(date.timestamp().duration_since(Timestamp::now())).ok())
        .filter(|delay| !delay.is_zero())
        .unwrap_or(DEFAULT_RATE_LIMIT_COOLDOWN)
}

/// Whether a failed attempt is worth repeating: a connection that could not
/// be made always is, other failures only for `repeatable` requests.
fn is_transient(error: &reqwest::Error, repeatable: bool) -> bool {
    error.is_connect() || (repeatable && (error.is_timeout() || error.is_request()))
}

/// Whether sending `method` twice has the same effect as sending it once.
fn is_repeatable(method: &Method) -> bool {
    !matches!(*method, Method::POST | Method::PUT | Method::PATCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn http(timeout: Duration) -> ProviderHttp {
        ProviderHttp::new(timeout, Arc::new(RateLimitCoordinator::new()))
            .expect("client builds")
            .with_retry_policy(RetryPolicy {
                max_retries: 2,
                base_delay: Duration::from_millis(1),
            })
    }

    fn host_of(server: &MockServer) -> String {
        reqwest::Url::parse(&server.uri())
            .expect("mock server uri")
            .host_str()
            .expect("host")
            .to_owned()
    }

    #[tokio::test]
    async fn too_many_requests_with_retry_after_becomes_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
            .expect(1)
            .mount(&server)
            .await;
        let http = http(Duration::from_secs(5));

        let error = http
            .send(http.client().get(server.uri()))
            .await
            .expect_err("429 is an error");

        assert_eq!(error.retry_after(), Some(Duration::from_secs(3600)));
        assert!(error.to_string().contains("rate limit exceeded"), "{error}");
        assert!(
            http.coordinator().remaining_cooldown(&host_of(&server)) > Duration::from_secs(3590)
        );
    }

    #[tokio::test]
    async fn too_many_requests_without_retry_after_waits_two_minutes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let http = http(Duration::from_secs(5));

        let error = http
            .send(http.client().get(server.uri()))
            .await
            .expect_err("429 is an error");

        assert_eq!(error.retry_after(), Some(DEFAULT_RATE_LIMIT_COOLDOWN));
    }

    #[tokio::test]
    async fn active_cooldown_rejects_without_sending() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let http = http(Duration::from_secs(5));
        http.coordinator()
            .update_cooldown(&host_of(&server), Duration::from_secs(300));

        let error = http
            .send(http.client().get(server.uri()))
            .await
            .expect_err("cooldown rejects");

        assert!(error.to_string().contains("cooldown active"), "{error}");
        assert!(error.retry_after().expect("rate limited") > Duration::from_secs(290));
    }

    #[tokio::test]
    async fn timeouts_are_retried_then_reported_as_transport_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
            .expect(3)
            .mount(&server)
            .await;
        let http = http(Duration::from_millis(50));

        let error = http
            .send(http.client().get(server.uri()))
            .await
            .expect_err("every attempt times out");

        assert!(
            matches!(&error, DebridError::Transport(e) if e.is_timeout()),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn timed_out_posts_and_puts_are_sent_once() {
        for verb in [Method::POST, Method::PUT] {
            let server = MockServer::start().await;
            Mock::given(method(verb.as_str()))
                .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
                .expect(1)
                .mount(&server)
                .await;
            let http = http(Duration::from_millis(50));

            let error = http
                .send(http.client().request(verb.clone(), server.uri()).body("x"))
                .await
                .expect_err("times out");

            assert!(
                matches!(&error, DebridError::Transport(e) if e.is_timeout()),
                "{verb}: {error:?}"
            );
            server.verify().await;
        }
    }

    #[tokio::test]
    async fn request_timeout_status_is_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(408))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let http = http(Duration::from_secs(5));

        let response = http
            .send(http.client().get(server.uri()))
            .await
            .expect("second attempt succeeds");

        assert_eq!(response.text().await.expect("body"), "ok");
    }

    #[tokio::test]
    async fn server_errors_are_returned_without_retrying() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let http = http(Duration::from_secs(5));

        let response = http
            .send(http.client().get(server.uri()))
            .await
            .expect("500 is a response");

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn retry_after_accepts_http_dates() {
        let mut headers = HeaderMap::new();
        let date = Timestamp::now() + jiff::SignedDuration::from_mins(10);
        let text = jiff::fmt::rfc2822::to_string(&date.to_zoned(jiff::tz::TimeZone::UTC))
            .expect("formats");
        headers.insert(RETRY_AFTER, HeaderValue::from_str(&text).expect("header"));

        let delay = retry_after_delay(&headers);

        assert!(
            delay > Duration::from_secs(590) && delay <= Duration::from_secs(600),
            "{delay:?}"
        );
    }

    #[test]
    fn retry_after_in_the_past_falls_back_to_default() {
        let mut headers = HeaderMap::new();
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Sun, 06 Nov 1994 08:49:37 GMT"),
        );

        assert_eq!(retry_after_delay(&headers), DEFAULT_RATE_LIMIT_COOLDOWN);
    }
}

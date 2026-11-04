//! GitHub release and published-advisory checks with cancellable hourly refreshes.

use std::collections::HashSet;
use std::time::Duration;

use reqwest::Client;
use reqwest::header::{ACCEPT, LINK};
use semver::{Op, Version, VersionReq};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use url::Url;

/// The delay between completed checks, including failed checks.
const CHECK_INTERVAL: Duration = Duration::from_hours(1);
/// Bound each GitHub request independently, including reading its response body.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const REPOSITORY_PATH: &str = "repos/pahorema/rd-client-rs/";

/// The running version and the most recently completed release/advisory check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct UpdateState {
    /// Running package version, prefixed with `v`.
    pub current: String,
    /// Latest release tag, or `None` before the first successful check.
    pub latest: Option<String>,
    /// Whether a published, non-withdrawn advisory affects the running version.
    pub insecure: bool,
}

/// A failed request, response, or update-check configuration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UpdateCheckError {
    /// An HTTP client could not be built or a request/response failed.
    #[error("update check request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// An API base or pagination URL could not be parsed.
    #[error("invalid update check URL: {0}")]
    Url(#[from] url::ParseError),
    /// The API base is not an HTTP(S) URL without credentials, query, or fragment.
    #[error("update check base URL must be an HTTP(S) URL without credentials, query, or fragment")]
    InvalidBaseUrl,
    /// The release response did not identify a release.
    #[error("latest GitHub release has an empty tag name")]
    EmptyReleaseTag,
    /// An advisory range could not be interpreted as semantic-version comparators.
    #[error("invalid advisory version range {range:?}: {source}")]
    AdvisoryRange {
        /// The unsupported or malformed range supplied by GitHub.
        range: String,
        /// The semantic-version parser error.
        #[source]
        source: semver::Error,
    },
    /// Pagination was malformed, cyclic, or left the repository advisory endpoint.
    #[error("invalid GitHub advisory pagination")]
    InvalidPagination,
}

/// A cheap-to-clone update checker independent of torrent processing or storage.
///
/// Clones share the last successful state. A failed check leaves that state intact,
/// including any previously confirmed vulnerability. The caller owns spawning and
/// joining [`Self::run`]; constructing a checker performs no network requests.
#[derive(Clone, Debug)]
pub struct UpdateChecker {
    client: Client,
    api_base: Url,
    version: Version,
    state: watch::Sender<UpdateState>,
}

impl UpdateChecker {
    /// Creates a checker using the public GitHub API and a 30-second request timeout.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be initialized.
    pub fn new() -> Result<Self, UpdateCheckError> {
        Self::with_base_url(Url::parse("https://api.github.com/")?)
    }

    /// Creates a checker with an alternate API root, such as a local HTTP test server.
    ///
    /// A base path is supported, with or without a trailing slash. Requests use
    /// `User-Agent: rd-client/<CARGO_PKG_VERSION>` and a 30-second timeout.
    ///
    /// # Errors
    /// Returns an error for a non-HTTP(S) base, credentials, query, fragment, or an
    /// HTTP client initialization failure.
    pub fn with_base_url(mut api_base: Url) -> Result<Self, UpdateCheckError> {
        if !matches!(api_base.scheme(), "http" | "https")
            || api_base.host_str().is_none()
            || !api_base.username().is_empty()
            || api_base.password().is_some()
            || api_base.query().is_some()
            || api_base.fragment().is_some()
        {
            return Err(UpdateCheckError::InvalidBaseUrl);
        }
        if !api_base.path().ends_with('/') {
            api_base.set_path(&format!("{}/", api_base.path()));
        }
        let client = Client::builder()
            .user_agent(concat!("rd-client/", env!("CARGO_PKG_VERSION")))
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        let version = Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("Cargo package versions are valid semantic versions");
        let (state, _) = watch::channel(UpdateState {
            current: format!("v{version}"),
            latest: None,
            insecure: false,
        });
        Ok(Self {
            client,
            api_base,
            version,
            state,
        })
    }

    /// Returns a snapshot without waiting for an in-progress HTTP request.
    #[must_use]
    pub fn state(&self) -> UpdateState {
        self.state.borrow().clone()
    }

    /// Subscribes to successful refreshes, initially containing the current snapshot.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<UpdateState> {
        self.state.subscribe()
    }

    /// Refreshes the release and every advisory page, atomically publishing the result.
    ///
    /// Only published, non-withdrawn advisories are considered. Their
    /// `vulnerabilities[].vulnerable_version_range` values are matched against the
    /// running semantic version, including comma-separated comparator intersections.
    /// Bare complete versions denote exact versions, not implicit caret ranges.
    ///
    /// # Errors
    /// Returns an error on HTTP, JSON, empty release tags, malformed advisory ranges,
    /// or invalid pagination. No state is changed unless the entire check succeeds.
    pub async fn check_once(&self) -> Result<UpdateState, UpdateCheckError> {
        let release_url = self
            .api_base
            .join(&format!("{REPOSITORY_PATH}releases/latest"))?;
        let release: Release = self
            .client
            .get(release_url)
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if release.tag_name.trim().is_empty() {
            return Err(UpdateCheckError::EmptyReleaseTag);
        }
        let insecure = self.check_advisories().await?;
        let state = UpdateState {
            current: format!("v{}", self.version),
            latest: Some(release.tag_name),
            insecure,
        };
        if state.latest != self.state.borrow().latest
            && state.latest.as_deref() != Some(state.current.as_str())
        {
            tracing::info!(latest = ?state.latest, "new release found on GitHub");
        }
        self.state.send_replace(state.clone());
        Ok(state)
    }

    /// Checks immediately, then waits one hour after each completed attempt.
    ///
    /// Failures are debug-logged and retried next hour rather than terminating the
    /// loop. Cancellation interrupts both active HTTP work and the hourly wait.
    pub async fn run(&self, cancel: CancellationToken) {
        self.run_with_interval(cancel, CHECK_INTERVAL).await;
    }

    async fn run_with_interval(&self, cancel: CancellationToken, interval: Duration) {
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                result = self.check_once() => {
                    if let Err(error) = result {
                        tracing::debug!(%error, "unable to check GitHub updates; retrying next interval");
                    }
                }
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(interval) => {}
            }
        }
    }

    async fn check_advisories(&self) -> Result<bool, UpdateCheckError> {
        let endpoint = self
            .api_base
            .join(&format!("{REPOSITORY_PATH}security-advisories"))?;
        let mut next = endpoint.clone();
        next.query_pairs_mut()
            .append_pair("state", "published")
            .append_pair("per_page", "100");
        let mut visited = HashSet::new();
        let mut insecure = false;
        loop {
            if !visited.insert(next.clone()) {
                return Err(UpdateCheckError::InvalidPagination);
            }
            let response = self
                .client
                .get(next.clone())
                .header(ACCEPT, "application/vnd.github+json")
                .send()
                .await?
                .error_for_status()?;
            let following = next_page(response.headers(), &next, &endpoint)?;
            let advisories: Vec<Advisory> = response.json().await?;
            for advisory in advisories {
                if advisory.state != "published" || advisory.withdrawn_at.is_some() {
                    continue;
                }
                for vulnerability in advisory.vulnerabilities.into_iter().flatten() {
                    if let Some(range) = vulnerability.vulnerable_version_range {
                        insecure |= matches_range(&range, &self.version)?;
                    }
                }
            }
            let Some(following) = following else {
                return Ok(insecure);
            };
            next = following;
        }
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

#[derive(Deserialize)]
struct Advisory {
    state: String,
    withdrawn_at: Option<String>,
    vulnerabilities: Option<Vec<Vulnerability>>,
}

#[derive(Deserialize)]
struct Vulnerability {
    vulnerable_version_range: Option<String>,
}

fn matches_range(range: &str, version: &Version) -> Result<bool, UpdateCheckError> {
    let mut requirement =
        VersionReq::parse(range).map_err(|source| UpdateCheckError::AdvisoryRange {
            range: range.to_owned(),
            source,
        })?;
    // GitHub bare version entries identify a single release; semver defaults to ^.
    for (text, comparator) in range.split(',').zip(&mut requirement.comparators) {
        if text
            .trim()
            .starts_with(|character: char| character.is_ascii_digit())
        {
            comparator.op = Op::Exact;
        }
    }
    Ok(requirement.matches(version))
}

fn next_page(
    headers: &reqwest::header::HeaderMap,
    current: &Url,
    endpoint: &Url,
) -> Result<Option<Url>, UpdateCheckError> {
    for value in headers.get_all(LINK) {
        let value = value
            .to_str()
            .map_err(|_| UpdateCheckError::InvalidPagination)?;
        for link in value.split(',') {
            let mut parts = link.trim().split(';');
            let target = parts
                .next()
                .ok_or(UpdateCheckError::InvalidPagination)?
                .trim();
            let is_next = parts.any(|part| {
                part.trim().split_once('=').is_some_and(|(name, value)| {
                    name.trim() == "rel"
                        && value
                            .trim()
                            .trim_matches('"')
                            .split_ascii_whitespace()
                            .any(|rel| rel == "next")
                })
            });
            if !is_next {
                continue;
            }
            let target = target
                .strip_prefix('<')
                .and_then(|value| value.strip_suffix('>'))
                .ok_or(UpdateCheckError::InvalidPagination)?;
            let url = current.join(target)?;
            if url.origin() != endpoint.origin()
                || url.path() != endpoint.path()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(UpdateCheckError::InvalidPagination);
            }
            return Ok(Some(url));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use super::*;

    const RELEASE_PATH: &str = "/repos/pahorema/rd-client-rs/releases/latest";
    const ADVISORIES_PATH: &str = "/repos/pahorema/rd-client-rs/security-advisories";

    fn checker(server: &MockServer) -> UpdateChecker {
        UpdateChecker::with_base_url(Url::parse(&server.uri()).unwrap()).unwrap()
    }

    fn advisory(range: &str) -> serde_json::Value {
        json!({
            "state": "published",
            "withdrawn_at": null,
            "vulnerabilities": [{"vulnerable_version_range": range}]
        })
    }

    async fn release(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path(RELEASE_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tag_name": "v99.0.0"})))
            .mount(server)
            .await;
    }

    async fn advisories(server: &MockServer, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path(ADVISORIES_PATH))
            .and(query_param("state", "published"))
            .and(query_param("per_page", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn latest_release_and_advisories_use_repository_endpoints_and_user_agent() {
        let server = MockServer::start().await;
        let ua = concat!("rd-client/", env!("CARGO_PKG_VERSION"));
        Mock::given(method("GET"))
            .and(path(RELEASE_PATH))
            .and(header("user-agent", ua))
            .and(header("accept", "application/vnd.github+json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tag_name": "v99.0.0"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(ADVISORIES_PATH))
            .and(header("user-agent", ua))
            .and(query_param("state", "published"))
            .and(query_param("per_page", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;
        let checker = checker(&server);
        assert_eq!(checker.state().latest, None);
        let updates = checker.subscribe();
        let state = checker.check_once().await.unwrap();
        assert_eq!(state.current, concat!("v", env!("CARGO_PKG_VERSION")));
        assert_eq!(state.latest.as_deref(), Some("v99.0.0"));
        assert!(!state.insecure);
        assert_eq!(*updates.borrow(), state);
        assert_eq!(checker.clone().state(), state);
    }

    #[tokio::test]
    async fn published_comma_range_marks_the_running_version_vulnerable() {
        let server = MockServer::start().await;
        release(&server).await;
        let version = env!("CARGO_PKG_VERSION");
        advisories(
            &server,
            json!([advisory(&format!(">= {version}, <= {version}"))]),
        )
        .await;
        assert!(checker(&server).check_once().await.unwrap().insecure);
    }

    #[tokio::test]
    async fn unrelated_draft_and_withdrawn_advisories_do_not_mark_version_insecure() {
        let server = MockServer::start().await;
        release(&server).await;
        let version = env!("CARGO_PKG_VERSION");
        let mut draft = advisory(&format!("= {version}"));
        draft["state"] = json!("draft");
        let mut withdrawn = advisory(&format!("= {version}"));
        withdrawn["withdrawn_at"] = json!("2026-01-01T00:00:00Z");
        advisories(
            &server,
            json!([
                advisory(&format!("< {version}")), draft, withdrawn,
                {"state": "published", "vulnerabilities": null},
                {"state": "published", "vulnerabilities": [{"vulnerable_version_range": null}]}
            ]),
        )
        .await;
        assert!(!checker(&server).check_once().await.unwrap().insecure);
    }

    #[tokio::test]
    async fn advisory_cursor_pagination_finds_a_later_vulnerable_range() {
        let server = MockServer::start().await;
        release(&server).await;
        let next = format!(
            "{}{ADVISORIES_PATH}?after=cursor&per_page=100&state=published",
            server.uri()
        );
        Mock::given(path(ADVISORIES_PATH))
            .and(query_param("state", "published"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Link", format!("<{next}>; rel=\"next\""))
                    .set_body_json(json!([])),
            )
            .with_priority(10)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path(ADVISORIES_PATH))
            .and(query_param("after", "cursor"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!([advisory(&format!(
                    "= {}",
                    env!("CARGO_PKG_VERSION")
                ))])),
            )
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        assert!(checker(&server).check_once().await.unwrap().insecure);
    }

    #[derive(Clone)]
    struct FailFirstRelease(Arc<AtomicUsize>);

    impl Respond for FailFirstRelease {
        fn respond(&self, _: &Request) -> ResponseTemplate {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"tag_name": "v99.0.0"}))
            }
        }
    }

    #[tokio::test]
    async fn immediate_loop_retries_after_failure_and_stops_when_cancelled() {
        let server = MockServer::start().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        Mock::given(path(RELEASE_PATH))
            .respond_with(FailFirstRelease(attempts.clone()))
            .mount(&server)
            .await;
        advisories(&server, json!([])).await;
        let checker = checker(&server);
        let mut updates = checker.subscribe();
        let cancel = CancellationToken::new();
        let job = {
            let checker = checker.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                checker
                    .run_with_interval(cancel, Duration::from_millis(10))
                    .await;
            })
        };
        tokio::time::timeout(Duration::from_secs(2), updates.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(attempts.load(Ordering::SeqCst) >= 2);
        assert_eq!(checker.state().latest.as_deref(), Some("v99.0.0"));
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), job)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn failed_advisory_refresh_retains_state_and_next_check_can_clear_insecure() {
        let server = MockServer::start().await;
        release(&server).await;
        advisories(
            &server,
            json!([advisory(&format!("= {}", env!("CARGO_PKG_VERSION")))]),
        )
        .await;
        let checker = checker(&server);
        let previous = checker.check_once().await.unwrap();
        assert!(previous.insecure);
        server.reset().await;
        release(&server).await;
        Mock::given(path(ADVISORIES_PATH))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        assert!(checker.check_once().await.is_err());
        assert_eq!(checker.state(), previous);
        server.reset().await;
        release(&server).await;
        advisories(&server, json!([])).await;
        assert!(!checker.check_once().await.unwrap().insecure);
    }

    #[tokio::test]
    async fn malformed_range_or_empty_release_is_recoverable_without_state_change() {
        let server = MockServer::start().await;
        release(&server).await;
        advisories(&server, json!([advisory("not a version range")])).await;
        let checker = checker(&server);
        let initial = checker.state();
        assert!(matches!(
            checker.check_once().await,
            Err(UpdateCheckError::AdvisoryRange { .. })
        ));
        assert_eq!(checker.state(), initial);
        server.reset().await;
        Mock::given(path(RELEASE_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tag_name": " "})))
            .mount(&server)
            .await;
        assert!(matches!(
            checker.check_once().await,
            Err(UpdateCheckError::EmptyReleaseTag)
        ));
        assert_eq!(checker.state(), initial);
        server.reset().await;
        release(&server).await;
        advisories(&server, json!([])).await;
        assert!(checker.check_once().await.is_ok());
    }

    #[tokio::test]
    async fn cancellation_interrupts_an_inflight_request() {
        let server = MockServer::start().await;
        Mock::given(path(RELEASE_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(10))
                    .set_body_json(json!({"tag_name": "v99.0.0"})),
            )
            .mount(&server)
            .await;
        let checker = checker(&server);
        let cancel = CancellationToken::new();
        let job = {
            let checker = checker.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { checker.run(cancel).await })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !server.received_requests().await.unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), job)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(checker.state().latest, None);
    }

    #[tokio::test]
    async fn cancelled_checker_does_not_start_a_request() {
        let server = MockServer::start().await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        checker(&server).run(cancel).await;
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn comparator_ranges_honor_boundaries_and_bare_exact_versions() {
        let version = Version::parse("1.2.3").unwrap();
        for (range, expected) in [
            (">= 1.0.0, < 2.0.0", true),
            (">= 1.2.3, <= 1.2.3", true),
            ("< 1.2.3", false),
            ("> 1.2.3", false),
            ("1.2.3", true),
            ("1.2.0", false),
            ("= 1.2.3", true),
        ] {
            assert_eq!(
                matches_range(range, &version).unwrap(),
                expected,
                "range {range}"
            );
        }
    }

    #[test]
    fn pagination_rejects_cycles_foreign_hosts_and_wrong_paths() {
        let endpoint =
            Url::parse("https://api.github.com/repos/pahorema/rd-client-rs/security-advisories")
                .unwrap();
        for target in [
            "https://example.com/advisories",
            "https://api.github.com/other",
        ] {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(LINK, format!("<{target}>; rel=\"next\"").parse().unwrap());
            assert!(matches!(
                next_page(&headers, &endpoint, &endpoint),
                Err(UpdateCheckError::InvalidPagination)
            ));
        }
    }

    #[tokio::test]
    async fn cyclic_pagination_fails_instead_of_looping_forever() {
        let server = MockServer::start().await;
        release(&server).await;
        let next = format!(
            "{}{ADVISORIES_PATH}?state=published&per_page=100",
            server.uri()
        );
        Mock::given(path(ADVISORIES_PATH))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Link", format!("<{next}>; rel=\"next\""))
                    .set_body_json(json!([])),
            )
            .expect(1)
            .mount(&server)
            .await;
        assert!(matches!(
            checker(&server).check_once().await,
            Err(UpdateCheckError::InvalidPagination)
        ));
    }
}

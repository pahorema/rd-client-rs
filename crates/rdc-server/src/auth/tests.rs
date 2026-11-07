use super::*;

use axum::{
    Extension, Router,
    body::{Body, to_bytes},
    middleware,
    routing::{get, post},
};
use rdc_store::User;
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::TempDir;
use tower::ServiceExt;

use crate::{http::AppBuilder, logging};

struct Fixture {
    directory: TempDir,
    store: Store,
    sessions: Sessions,
    user: User,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path().join("test.db")).await.unwrap();
        let user = store.users().create("alice", "Passw0rd!").await.unwrap();
        let sessions = Sessions::open(store.clone(), &directory.path().join("session.key"))
            .await
            .unwrap();
        Self {
            directory,
            store,
            sessions,
            user,
        }
    }

    fn router(&self, base: &str, disabled: Arc<AtomicBool>) -> Router {
        let policy = AuthPolicy::new(
            self.sessions.clone(),
            Arc::new(move || disabled.load(Ordering::Relaxed)),
        );
        let protected = Router::new()
            .route(
                "/api/private",
                get(|Extension(user): Extension<RequestUser>| async move {
                    match user {
                        RequestUser::Authenticated { user_id } => user_id,
                        RequestUser::AuthenticationDisabled => "anonymous".to_owned(),
                    }
                }),
            )
            .route("/api/logout", post(logout))
            .route_layer(middleware::from_fn_with_state(policy, require_auth));
        let (_, logs) = logging::subscriber(io::sink);
        AppBuilder::new(base, logs)
            .route("/api/login", post(login))
            .merge(protected, &["/api/private", "/api/logout"])
            .build(self.sessions.clone())
    }

    fn cookie_at(&self, issued: u64) -> HeaderValue {
        let digest = Sha256::digest(self.user.security_stamp.as_ref().unwrap().as_bytes()).into();
        let set_cookie = self
            .sessions
            .issue(
                &self.user.id,
                &digest,
                issued,
                &root_scope(),
            )
            .unwrap();
        cookie(&set_cookie)
    }
}

#[tokio::test]
async fn sessions_expose_the_requesting_account_name() {
    let fixture = Fixture::new().await;
    let signed_in = RequestUser::Authenticated {
        user_id: fixture.user.id.clone(),
    };
    assert_eq!(
        fixture.sessions.user_name(&signed_in).await.unwrap().as_deref(),
        Some("alice")
    );
    let unknown = RequestUser::Authenticated {
        user_id: "missing".to_owned(),
    };
    assert_eq!(fixture.sessions.user_name(&unknown).await.unwrap(), None);
    assert_eq!(
        fixture
            .sessions
            .user_name(&RequestUser::AuthenticationDisabled)
            .await
            .unwrap()
            .as_deref(),
        Some("alice")
    );
    let directory = tempfile::tempdir().unwrap();
    let empty = Store::open(directory.path().join("empty.db"))
        .await
        .unwrap();
    let sessions = Sessions::open(empty, &directory.path().join("session.key"))
        .await
        .unwrap();
    assert_eq!(
        sessions
            .user_name(&RequestUser::AuthenticationDisabled)
            .await
            .unwrap(),
        None
    );
}

async fn login(
    State(sessions): State<Sessions>,
    scope: CookieScope,
) -> Response {
    let value = sessions
        .login("alice", "Passw0rd!", &scope)
        .await
        .unwrap()
        .unwrap();
    ([(header::SET_COOKIE, value)], StatusCode::OK).into_response()
}

async fn logout(
    State(sessions): State<Sessions>,
    scope: CookieScope,
) -> Response {
    (
        [(header::SET_COOKIE, sessions.logout(&scope).unwrap())],
        StatusCode::OK,
    )
        .into_response()
}

fn root_scope() -> CookieScope {
    CookieScope {
        base_path: RequestBasePath(String::new()),
        secure: false,
    }
}

fn enabled() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn cookie(value: &HeaderValue) -> HeaderValue {
    HeaderValue::from_str(value.to_str().unwrap().split(';').next().unwrap()).unwrap()
}

fn headers(value: HeaderValue) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, value);
    headers
}

async fn request(router: Router, method: &str, path: &str, value: Option<HeaderValue>) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    if let Some(value) = value {
        request.headers_mut().insert(header::COOKIE, value);
    }
    router.oneshot(request).await.unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn login_cookie_has_persistent_attributes_and_the_request_mount_path() {
    let fixture = Fixture::new().await;
    let router = fixture.router("/downloads/client", enabled());
    for (path, expected_path) in [
        ("/api/login", "Path=/"),
        ("/downloads/client/api/login", "Path=/downloads/client/"),
    ] {
        let response = request(router.clone(), "POST", path, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = &response.headers()[header::SET_COOKIE];
        let attributes: Vec<_> = set_cookie.to_str().unwrap().split("; ").collect();
        assert!(attributes.contains(&"HttpOnly"));
        assert!(attributes.contains(&"SameSite=Strict"));
        assert!(!attributes.contains(&"Secure"), "plain HTTP request");
        assert!(attributes.contains(&expected_path));
        assert!(attributes.contains(&"Max-Age=1209600"));
        let value = cookie(set_cookie);
        assert!(fixture.sessions.is_authenticated(&headers(value)).await);
        let payload = BASE64URL_NOPAD
            .decode(
                attributes[0]
                    .strip_prefix("SID=")
                    .unwrap()
                    .split('.')
                    .next()
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();
        assert!(
            !payload
                .windows(fixture.user.security_stamp.as_ref().unwrap().len())
                .any(|part| part == fixture.user.security_stamp.as_ref().unwrap().as_bytes())
        );
    }
}

#[tokio::test]
async fn cookies_are_secure_when_a_proxy_reports_https() {
    let fixture = Fixture::new().await;
    let router = fixture.router("", enabled());
    for (proto, secure) in [("https", true), ("HTTPS, http", true), ("http", false)] {
        let login = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header("x-forwarded-proto", proto)
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(login).await.unwrap();
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert_eq!(
            set_cookie.split("; ").any(|part| part == "Secure"),
            secure,
            "{proto}: {set_cookie}"
        );
    }
}

#[tokio::test]
async fn valid_cookie_passes_and_inserts_the_authenticated_account() {
    let fixture = Fixture::new().await;
    let router = fixture.router("/mount", enabled());
    let response = request(
        router,
        "GET",
        "/mount/api/private",
        Some(fixture.cookie_at(now())),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(body(response).await, fixture.user.id);
}

#[tokio::test]
async fn missing_tampered_expired_future_and_malformed_cookies_return_empty_forbidden() {
    let fixture = Fixture::new().await;
    let router = fixture.router("", enabled());
    let valid = fixture.cookie_at(now());
    let tampered = format!("{}x", valid.to_str().unwrap()).parse().unwrap();
    for value in [
        None,
        Some(tampered),
        Some(fixture.cookie_at(now() - LIFETIME)),
        Some(fixture.cookie_at(now() + 3600)),
        Some(HeaderValue::from_static("SID=not-a-cookie")),
        Some(HeaderValue::from_static("SID=...")),
        Some(HeaderValue::from_static("other=valid")),
    ] {
        let response = request(router.clone(), "GET", "/api/private", value).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(body(response).await, "");
    }
}

#[tokio::test]
async fn disabled_switch_is_live_and_passes_without_a_cookie() {
    let fixture = Fixture::new().await;
    let disabled = enabled();
    let router = fixture.router("", disabled.clone());
    disabled.store(true, Ordering::Relaxed);
    let response = request(router.clone(), "GET", "/api/private", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(body(response).await, "anonymous");
    disabled.store(false, Ordering::Relaxed);
    assert_eq!(
        request(router, "GET", "/api/private", None).await.status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn credential_changes_and_invalidation_reject_previously_cached_cookies() {
    let fixture = Fixture::new().await;
    for (user_name, password) in [("alice", "Changed1!"), ("bob", "")] {
        let current = if user_name == "alice" {
            "Passw0rd!"
        } else {
            "Changed1!"
        };
        let old = fixture
            .sessions
            .login("alice", current, &root_scope())
            .await
            .unwrap()
            .unwrap();
        let old = headers(cookie(&old));
        assert!(fixture.sessions.is_authenticated(&old).await);
        let user = fixture
            .store
            .users()
            .update_credentials(&fixture.user.id, current, user_name, password)
            .await
            .unwrap();
        fixture.sessions.clone().invalidate(&user.id).await;
        assert!(!fixture.sessions.is_authenticated(&old).await);
        let response = request(
            fixture.router("", enabled()),
            "GET",
            "/api/private",
            old.get(header::COOKIE).cloned(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(body(response).await, "");
    }
}

#[tokio::test]
async fn sliding_refresh_renews_after_half_lifetime_and_preserves_logout() {
    let fixture = Fixture::new().await;
    let router = fixture.router("/mount", enabled());
    let old = fixture.cookie_at(now() - LIFETIME / 2 - 100);
    let response = request(
        router.clone(),
        "GET",
        "/mount/api/private",
        Some(old.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let refreshed = &response.headers()[header::SET_COOKIE];
    assert!(refreshed.to_str().unwrap().contains("Path=/mount/"));
    assert_ne!(cookie(refreshed), old);
    let session = fixture
        .sessions
        .authenticate(&headers(cookie(refreshed)), now())
        .await
        .unwrap();
    assert!(!session.refresh);
    let midpoint_time = now();
    let midpoint = fixture.cookie_at(midpoint_time - LIFETIME / 2);
    assert!(
        !fixture
            .sessions
            .authenticate(&headers(midpoint), midpoint_time)
            .await
            .unwrap()
            .refresh
    );
    let response = request(router, "POST", "/mount/api/logout", Some(old)).await;
    assert_eq!(
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .count(),
        1
    );
    let expired = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(expired.starts_with("SID=;"));
    assert!(expired.contains("Max-Age=0"));
    assert!(expired.contains("Path=/mount/"));
}

#[tokio::test]
async fn key_reload_preserves_cookies_and_new_unix_keys_are_private() {
    let fixture = Fixture::new().await;
    let path = fixture.directory.path().join("session.key");
    let original = std::fs::read(&path).unwrap();
    assert_eq!(original.len(), 32);
    let sessions = Sessions::open(fixture.store.clone(), &path).await.unwrap();
    assert!(
        sessions
            .is_authenticated(&headers(fixture.cookie_at(now())))
            .await
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(format!("{sessions:?}"), "Sessions { .. }");
}

#[tokio::test]
async fn concurrent_key_initializers_share_one_key_and_invalid_keys_are_not_replaced() {
    let fixture = Fixture::new().await;
    let path = fixture.directory.path().join("concurrent.key");
    let (first, second) = tokio::join!(
        Sessions::open(fixture.store.clone(), &path),
        Sessions::open(fixture.store.clone(), &path)
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    let set_cookie = first
        .login("alice", "Passw0rd!", &root_scope())
        .await
        .unwrap()
        .unwrap();
    assert!(second.is_authenticated(&headers(cookie(&set_cookie))).await);
    let bad_path = fixture.directory.path().join("bad.key");
    std::fs::write(&bad_path, b"invalid").unwrap();
    assert!(matches!(
        Sessions::open(fixture.store.clone(), &bad_path).await,
        Err(AuthError::KeyFile(_))
    ));
    assert_eq!(std::fs::read(&bad_path).unwrap(), b"invalid");
}

#[tokio::test]
async fn blank_credentials_fail_without_accessing_the_closed_store() {
    let fixture = Fixture::new().await;
    fixture.store.close().await;
    for (name, password) in [("", "Passw0rd!"), (" \t", "Passw0rd!"), ("alice", "")] {
        assert!(
            fixture
                .sessions
                .login(name, password, &root_scope())
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn wrong_credentials_fail_and_malformed_mount_paths_are_rejected() {
    let fixture = Fixture::new().await;
    let root = root_scope();
    for (name, password) in [("alice", "wrong"), ("missing", "Passw0rd!")] {
        assert!(
            fixture
                .sessions
                .login(name, password, &root)
                .await
                .unwrap()
                .is_none()
        );
    }
    for path in ["relative", "/mount; HttpOnly", "/mount\r\n"] {
        assert!(matches!(
            fixture.sessions.logout(&CookieScope {
                base_path: RequestBasePath(path.to_owned()),
                secure: false,
            }),
            Err(AuthError::CookiePath)
        ));
    }
    let expired = fixture.sessions.logout(&root).unwrap();
    assert!(
        !fixture
            .sessions
            .is_authenticated(&headers(cookie(&expired)))
            .await
    );
}

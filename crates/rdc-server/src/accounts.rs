//! Account setup, session and credential endpoints for the web UI.

use axum::{
    Extension, Json, Router,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rdc_store::UserError;
use serde::Deserialize;

use crate::{
    app::{AppState, CaseInsensitive, internal, required},
    auth::{CookieScope, RequestUser},
    http::ApiError,
};

/// Anonymous session and initial-account endpoint paths.
pub(crate) const PUBLIC_PATHS: &[&str] = &[
    "/api/authentication/isloggedin",
    "/api/authentication/create",
    "/api/authentication/login",
    "/api/authentication/logout",
];

/// Account and provider setup paths requiring the application authorization policy.
pub(crate) const PROTECTED_PATHS: &[&str] = &[
    "/api/authentication/setupprovider",
    "/api/authentication/update",
];

/// Anonymous account creation and session routes.
pub(crate) fn public_routes() -> Router<AppState> {
    Router::new()
        .route(PUBLIC_PATHS[0], get(is_logged_in))
        .route(PUBLIC_PATHS[1], post(create))
        .route(PUBLIC_PATHS[2], post(login))
        .route(PUBLIC_PATHS[3], post(logout))
}

/// Routes that must be wrapped with the shared authentication middleware.
pub(crate) fn protected_routes() -> Router<AppState> {
    Router::new()
        .route(PROTECTED_PATHS[0], post(setup_provider))
        .route(PROTECTED_PATHS[1], post(update))
}

#[derive(Deserialize)]
struct Credentials {
    #[serde(rename = "username")]
    user_name: Option<String>,
    password: Option<String>,
    /// Authorizes a credential change.
    #[serde(rename = "currentpassword")]
    current_password: Option<String>,
}

#[derive(Deserialize)]
struct ProviderSetup {
    provider: i32,
    token: Option<String>,
}

async fn is_logged_in(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if state.policy.is_disabled() || state.sessions.is_authenticated(&headers).await {
        return Ok(StatusCode::OK.into_response());
    }
    if state
        .store
        .users()
        .first()
        .await
        .map_err(internal)?
        .is_none()
    {
        Ok(ApiError::new(StatusCode::PAYMENT_REQUIRED, "Setup required").into_response())
    } else {
        Ok(StatusCode::FORBIDDEN.into_response())
    }
}

async fn create(
    State(state): State<AppState>,
    scope: CookieScope,
    input: Result<Json<Option<CaseInsensitive<Credentials>>>, JsonRejection>,
) -> Result<Response, ApiError> {
    let credentials = required(input)?.0;
    if state
        .store
        .users()
        .first()
        .await
        .map_err(internal)?
        .is_some()
    {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let user_name = credentials.user_name.as_deref().unwrap_or_default();
    let password = credentials.password.as_deref().unwrap_or_default();
    if user_name.is_empty() || password.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Invalid UserName or Password",
        ));
    }
    state
        .store
        .users()
        .create(user_name, password)
        .await
        .map_err(user_error)?;
    let cookie = state
        .sessions
        .login(user_name, password, &scope)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("Account created but sign-in failed"))?;
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::OK).into_response())
}

async fn login(
    State(state): State<AppState>,
    scope: CookieScope,
    input: Result<Json<Option<CaseInsensitive<Credentials>>>, JsonRejection>,
) -> Result<Response, ApiError> {
    let credentials = required(input)?.0;
    if state
        .store
        .users()
        .first()
        .await
        .map_err(internal)?
        .is_none()
    {
        return Ok(StatusCode::PAYMENT_REQUIRED.into_response());
    }
    let user_name = credentials.user_name.as_deref().unwrap_or_default();
    let password = credentials.password.as_deref().unwrap_or_default();
    if user_name.is_empty() || password.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Invalid credentials",
        ));
    }
    let Some(cookie) = state
        .sessions
        .login(user_name, password, &scope)
        .await
        .map_err(internal)?
    else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Invalid credentials",
        ));
    };
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::OK).into_response())
}

async fn logout(
    State(state): State<AppState>,
    scope: CookieScope,
) -> Result<Response, ApiError> {
    let cookie = state.sessions.logout(&scope).map_err(internal)?;
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::OK).into_response())
}

async fn setup_provider(
    State(state): State<AppState>,
    input: Result<Json<Option<CaseInsensitive<ProviderSetup>>>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let request = required(input)?.0;
    if !state.engine.settings().provider.api_key.is_empty() {
        return Ok(StatusCode::FORBIDDEN);
    }
    state
        .engine
        .update_settings(vec![
            (
                "Provider:Provider".to_owned(),
                Some(request.provider.to_string()),
            ),
            ("Provider:ApiKey".to_owned(), request.token),
        ])
        .await
        .map_err(internal)?;
    Ok(StatusCode::OK)
}

/// Changes the credentials of the signed-in account after checking its current password.
/// With authentication disabled there is no signed-in account, so the only account is
/// changed; the current password still proves ownership.
async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    input: Result<Json<Option<CaseInsensitive<Credentials>>>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let credentials = required(input)?.0;
    let user_name = credentials.user_name.as_deref().unwrap_or_default();
    let password = credentials.password.as_deref().unwrap_or_default();
    if user_name.is_empty() || password.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Invalid UserName or Password",
        ));
    }
    let users = state.store.users();
    let user_id = match user {
        RequestUser::Authenticated { user_id } => user_id,
        RequestUser::AuthenticationDisabled => {
            users
                .first()
                .await
                .map_err(internal)?
                .ok_or_else(|| user_error(UserError::NoUser))?
                .id
        }
    };
    let current = credentials.current_password.as_deref().unwrap_or_default();
    let user = users
        .update_credentials(&user_id, current, user_name, password)
        .await
        .map_err(user_error)?;
    state.sessions.invalidate(&user.id).await;
    Ok(StatusCode::OK)
}

fn user_error(error: UserError) -> ApiError {
    match error {
        UserError::Rejected(reason) => ApiError::new(StatusCode::BAD_REQUEST, reason),
        UserError::IncorrectPassword => ApiError::new(StatusCode::BAD_REQUEST, error.to_string()),
        UserError::NoUser => ApiError::new(StatusCode::BAD_REQUEST, "No account exists"),
        UserError::AccountExists => ApiError::new(StatusCode::FORBIDDEN, ""),
        other => internal(other),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, header},
    };
    use rdc_engine::testing::TestEngine;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::{
        app,
        auth::{AuthPolicy, Sessions},
        logging,
    };

    struct Fixture {
        test: TestEngine,
        router: Router,
        disabled: Arc<AtomicBool>,
    }

    impl Fixture {
        async fn new(base: &str) -> Self {
            let test = TestEngine::new().await.expect("isolated engine starts");
            let sessions = Sessions::open(
                test.store.clone(),
                &test.tempdir.path().join("sessions.key"),
            )
            .await
            .expect("session key initializes");
            let disabled = Arc::new(AtomicBool::new(false));
            let flag = disabled.clone();
            let policy = AuthPolicy::new(
                sessions.clone(),
                Arc::new(move || flag.load(Ordering::Relaxed)),
            );
            let (_, logs) = logging::subscriber(io::sink);
            let router = app::build(
                app::Backend {
                    engine: test.engine.clone(),
                    store: test.store.clone(),
                    sessions,
                    policy,
                },
                base,
                logs,
                tokio_util::sync::CancellationToken::new(),
            );
            Self {
                test,
                router,
                disabled,
            }
        }

        async fn send(
            &self,
            method: &str,
            path: &str,
            value: Option<Value>,
            cookie: Option<&str>,
        ) -> Response {
            let mut request = Request::builder().method(method).uri(path);
            if value.is_some() {
                request = request.header(header::CONTENT_TYPE, "application/json");
            }
            if let Some(cookie) = cookie {
                request = request.header(header::COOKIE, cookie);
            }
            let body = value.map_or_else(Body::empty, |value| Body::from(value.to_string()));
            self.router
                .clone()
                .oneshot(request.body(body).expect("valid request"))
                .await
                .expect("router responds")
        }
    }

    fn cookie(response: &Response) -> String {
        response.headers()[header::SET_COOKIE]
            .to_str()
            .expect("cookie header is text")
            .split(';')
            .next()
            .expect("cookie has first part")
            .to_owned()
    }

    async fn text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body is readable");
        String::from_utf8(bytes.to_vec()).expect("body is UTF-8")
    }

    #[tokio::test]
    async fn account_creation_login_logout_and_status_follow_session_policy() {
        let fixture = Fixture::new("/client").await;
        let root = "/client/Api/Authentication/";
        let status = fixture
            .send("GET", &format!("{root}IsLoggedIn"), None, None)
            .await;
        assert_eq!(status.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(text(status).await, "Setup required");
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}Login"),
                    Some(json!({
                        "userName":"alice","password":"Passw0rd!"
                    })),
                    None
                )
                .await
                .status(),
            StatusCode::PAYMENT_REQUIRED
        );
        assert_eq!(
            fixture
                .send("POST", &format!("{root}Create"), Some(Value::Null), None)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        let malformed = Request::builder()
            .method("POST")
            .uri(format!("{root}Create"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{"))
            .expect("valid request");
        let malformed = fixture
            .router
            .clone()
            .oneshot(malformed)
            .await
            .expect("router responds");
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        assert_eq!(text(malformed).await, "");
        let weak = fixture
            .send(
                "POST",
                &format!("{root}Create"),
                Some(json!({
                    "userName":"alice", "password":"weak"
                })),
                None,
            )
            .await;
        assert_eq!(weak.status(), StatusCode::BAD_REQUEST);
        let created = fixture
            .send(
                "POST",
                &format!("{root}Create"),
                Some(json!({
                    "USERNAME":"alice", "pAsSwOrD":"Passw0rd!"
                })),
                None,
            )
            .await;
        assert_eq!(created.status(), StatusCode::OK);
        assert!(
            created.headers()[header::SET_COOKIE]
                .to_str()
                .expect("cookie")
                .contains("Path=/client/")
        );
        let sid = cookie(&created);
        assert_eq!(
            fixture
                .send("GET", &format!("{root}IsLoggedIn"), None, Some(&sid))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}Create"),
                    Some(json!({
                        "userName":"other", "password":"Passw0rd!"
                    })),
                    None
                )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let signed_out = fixture
            .send("POST", &format!("{root}Logout"), None, Some(&sid))
            .await;
        assert_eq!(signed_out.status(), StatusCode::OK);
        assert!(
            signed_out.headers()[header::SET_COOKIE]
                .to_str()
                .expect("cookie")
                .contains("Max-Age=0")
        );
        assert_eq!(
            fixture
                .send("GET", &format!("{root}IsLoggedIn"), None, None)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let invalid = fixture
            .send(
                "POST",
                &format!("{root}Login"),
                Some(json!({
                    "userName":"alice","password":"wrong"
                })),
                None,
            )
            .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(text(invalid).await, "Invalid credentials");
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}Login"),
                    Some(json!({
                        "UserName":"alice","PASSWORD":"Passw0rd!"
                    })),
                    None
                )
                .await
                .status(),
            StatusCode::OK
        );
        fixture.disabled.store(true, Ordering::Relaxed);
        assert_eq!(
            fixture
                .send("GET", &format!("{root}IsLoggedIn"), None, None)
                .await
                .status(),
            StatusCode::OK
        );
        fixture.test.engine.shutdown().await;
    }

    #[tokio::test]
    async fn provider_setup_requires_auth_and_rejects_replacing_existing_key() {
        let fixture = Fixture::new("").await;
        let root = "/api/authentication/";
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}SetupProvider"),
                    Some(json!({
                        "provider":1,"token":"secret"
                    })),
                    None
                )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let created = fixture
            .send(
                "POST",
                &format!("{root}Create"),
                Some(json!({
                    "userName":"alice","password":"Passw0rd!"
                })),
                None,
            )
            .await;
        let sid = cookie(&created);
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}SetupProvider"),
                    Some(Value::Null),
                    Some(&sid)
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}SetupProvider"),
                    Some(json!({
                        "provider":1,"token":"secret"
                    })),
                    Some(&sid)
                )
                .await
                .status(),
            StatusCode::OK
        );
        let settings = fixture
            .test
            .store
            .settings()
            .load()
            .await
            .expect("settings load");
        assert_eq!(settings.provider.provider.to_string(), "AllDebrid");
        assert_eq!(settings.provider.api_key, "secret");
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}SetupProvider"),
                    Some(json!({
                        "provider":2,"token":"other"
                    })),
                    Some(&sid)
                )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        fixture.test.engine.shutdown().await;
    }

    #[tokio::test]
    async fn credential_update_needs_the_current_password_and_invalidates_old_cookie() {
        let fixture = Fixture::new("").await;
        let root = "/api/authentication/";
        let created = fixture
            .send(
                "POST",
                &format!("{root}Create"),
                Some(json!({
                    "userName":"alice","password":"Passw0rd!"
                })),
                None,
            )
            .await;
        let sid = cookie(&created);
        let invalid = fixture
            .send(
                "POST",
                &format!("{root}Update"),
                Some(json!({
                    "currentPassword":"Passw0rd!",
                    "userName":"invalid name", "password":"NewPassw0rd!"
                })),
                Some(&sid),
            )
            .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_ne!(text(invalid).await, "");
        for current in [None, Some("wrong-Passw0rd")] {
            let mut change = json!({"userName":"newname","password":"NewPassw0rd!"});
            if let Some(current) = current {
                change["currentPassword"] = current.into();
            }
            let refused = fixture
                .send("POST", &format!("{root}Update"), Some(change), Some(&sid))
                .await;
            assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
            assert_eq!(text(refused).await, "The current password is incorrect.");
        }
        assert_eq!(
            fixture
                .send(
                    "GET",
                    &format!("{root}IsLoggedIn"),
                    None,
                    Some(&sid)
                )
                .await
                .status(),
            StatusCode::OK,
            "refused changes keep the session"
        );
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}Update"),
                    Some(json!({
                        "currentPassword":"Passw0rd!",
                        "userName":"newname","password":"NewPassw0rd!"
                    })),
                    Some(&sid)
                )
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            fixture
                .send("GET", &format!("{root}IsLoggedIn"), None, Some(&sid))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}Login"),
                    Some(json!({
                        "userName":"alice","password":"Passw0rd!"
                    })),
                    None
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            fixture
                .send(
                    "POST",
                    &format!("{root}Login"),
                    Some(json!({
                        "userName":"newname","password":"NewPassw0rd!"
                    })),
                    None
                )
                .await
                .status(),
            StatusCode::OK
        );
        fixture.test.engine.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_setup_requests_create_a_single_account() {
        let fixture = Arc::new(Fixture::new("").await);
        let mut requests = tokio::task::JoinSet::new();
        for name in ["owner", "intruder1", "intruder2", "intruder3"] {
            let fixture = fixture.clone();
            requests.spawn(async move {
                fixture
                    .send(
                        "POST",
                        "/api/authentication/create",
                        Some(json!({"userName": name, "password": "Passw0rd!"})),
                        None,
                    )
                    .await
                    .status()
            });
        }
        let statuses = requests.join_all().await;
        assert_eq!(
            statuses.iter().filter(|status| **status == StatusCode::OK).count(),
            1,
            "{statuses:?}"
        );
        assert!(
            statuses
                .iter()
                .all(|status| matches!(*status, StatusCode::OK | StatusCode::FORBIDDEN)),
            "{statuses:?}"
        );
        let mut accounts = 0;
        for name in ["owner", "intruder1", "intruder2", "intruder3"] {
            let outcome = fixture
                .test
                .store
                .users()
                .verify_login(name, "Passw0rd!")
                .await
                .expect("login check");
            accounts += usize::from(matches!(outcome, rdc_store::LoginOutcome::Success(_)));
        }
        assert_eq!(accounts, 1, "only the winning name can sign in");
        fixture.test.engine.shutdown().await;
    }
}

use rdc_synology::{Credentials, SessionProvider};
use serde_json::json;
use tokio::task::JoinSet;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer};

use crate::common::{failure, logged_in, mount_info, ok, ok_without_data};

fn credentials(server: &MockServer, password: &str) -> Credentials {
    Credentials::new(server.uri(), "user", password)
}

/// Expects `times` logins with `password`, each answered with `sid`.
async fn expect_login(server: &MockServer, password: &str, sid: &str, times: u64) {
    Mock::given(method("POST"))
        .and(path("/webapi/entry.cgi"))
        .and(body_string_contains("method=login"))
        .and(body_string_contains(format!("passwd={password}&")))
        .respond_with(ok(&json!({"sid": sid})))
        .up_to_n_times(times)
        .expect(times)
        .mount(server)
        .await;
}

/// Expects `times` logouts of the session `sid`.
async fn expect_logout(server: &MockServer, sid: &str, times: u64) {
    Mock::given(method("GET"))
        .and(path("/webapi/entry.cgi"))
        .and(query_param("method", "logout"))
        .and(query_param("_sid", sid))
        .respond_with(ok_without_data())
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn concurrent_callers_log_in_once_and_share_one_session() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    expect_login(&server, "pass", "sid-1", 1).await;
    let provider = SessionProvider::new();
    let credentials = credentials(&server, "pass");

    let mut callers = JoinSet::new();
    for _ in 0..50 {
        let provider = provider.clone();
        let credentials = credentials.clone();
        callers.spawn(async move { provider.session(&credentials).await });
    }
    let sessions: Vec<_> = callers
        .join_all()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(sessions.len(), 50);
    assert!(sessions.iter().all(|session| *session == sessions[0]));
}

#[tokio::test]
async fn same_credentials_reuse_the_session_and_changed_credentials_log_in_again() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    expect_login(&server, "pass", "sid-1", 1).await;
    expect_login(&server, "changed", "sid-2", 1).await;
    expect_logout(&server, "sid-1", 1).await;
    let provider = SessionProvider::new();

    let first = provider
        .session(&credentials(&server, "pass"))
        .await
        .unwrap();
    let again = provider
        .session(&credentials(&server, "pass"))
        .await
        .unwrap();
    let changed = provider
        .session(&credentials(&server, "changed"))
        .await
        .unwrap();

    assert_eq!(first, again);
    assert_ne!(first, changed);
}

#[tokio::test]
async fn invalidate_then_session_logs_in_again_and_logs_out_the_stale_session() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    expect_login(&server, "pass", "sid-1", 1).await;
    expect_login(&server, "pass", "sid-2", 1).await;
    expect_logout(&server, "sid-1", 1).await;
    let provider = SessionProvider::new();
    let credentials = credentials(&server, "pass");

    let first = provider.session(&credentials).await.unwrap();
    provider.invalidate(Some(&first)).await;
    let second = provider.session(&credentials).await.unwrap();

    assert_ne!(first, second);
}

#[tokio::test]
async fn invalidate_with_an_already_replaced_session_keeps_the_current_one() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    expect_login(&server, "pass", "sid-1", 1).await;
    expect_logout(&server, "sid-1", 0).await;
    let other_server = MockServer::start().await;
    let stale = logged_in(&other_server).await;
    let provider = SessionProvider::new();
    let credentials = credentials(&server, "pass");

    let current = provider.session(&credentials).await.unwrap();
    provider.invalidate(Some(&stale)).await;
    let after_stale_invalidate = provider.session(&credentials).await.unwrap();

    assert_eq!(current, after_stale_invalidate);
}

#[tokio::test]
async fn invalidate_without_a_session_drops_the_current_one() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    expect_login(&server, "pass", "sid-1", 1).await;
    expect_login(&server, "pass", "sid-2", 1).await;
    expect_logout(&server, "sid-1", 1).await;
    let provider = SessionProvider::new();
    let credentials = credentials(&server, "pass");

    let first = provider.session(&credentials).await.unwrap();
    provider.invalidate(None).await;
    let second = provider.session(&credentials).await.unwrap();

    assert_ne!(first, second);
}

#[tokio::test]
async fn failed_login_is_reported_and_not_cached() {
    let server = MockServer::start().await;
    mount_info(&server).await;
    Mock::given(method("POST"))
        .respond_with(failure(400))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    expect_login(&server, "pass", "sid-1", 1).await;
    let provider = SessionProvider::new();
    let credentials = credentials(&server, "pass");

    let error = provider.session(&credentials).await.unwrap_err();
    let session = provider.session(&credentials).await;

    assert_eq!(error.api_code(), Some(400));
    assert!(session.is_ok(), "{session:?}");
}

#[test]
fn credentials_debug_output_redacts_the_password() {
    let credentials = Credentials::new("http://nas:5000", "user", "hunter2");

    let debug = format!("{credentials:?}");

    assert!(!debug.contains("hunter2"), "{debug}");
}

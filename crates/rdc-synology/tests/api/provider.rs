use rdc_synology::{Credentials, SessionProvider};
use serde_json::json;
use tokio::task::JoinSet;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer};

use crate::common::{failure, mount_info, ok};

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

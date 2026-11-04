//! TorBox client against a mock API.

mod common;

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use rdc_debrid::{DebridError, ProviderHttp, RateLimitCoordinator, RetryPolicy, TorBox};
use wiremock::matchers::{body_json_string, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Retries transient errors twice without waiting.
const FAST_RETRY: RetryPolicy = RetryPolicy {
    max_retries: 2,
    base_delay: Duration::ZERO,
};

async fn client() -> (MockServer, TorBox) {
    let server = MockServer::start().await;
    let client = TorBox::new(&common::config(), common::http())
        .with_base_url(&server.uri())
        .with_error_retry_policy(FAST_RETRY);
    (server, client)
}

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "application/json")
}

async fn mount_user(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/user/me"))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn user_reports_email_and_plan_end() {
    let (server, client) = client().await;
    Mock::given(method("GET"))
        .and(path("/user/me"))
        .and(query_param("settings", "false"))
        .and(header("authorization", "Bearer test-api-key"))
        .respond_with(json(
            r#"{"success":true,"error":null,"detail":"ok","data":
                {"email":"me@example.com","plan":2,"premium_expires_at":"2025-11-01T00:00:00Z"}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let user = client.user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("me@example.com"));
    assert_eq!(
        user.expiration,
        Some(Timestamp::from_second(1_761_955_200).expect("ts"))
    );
}

#[tokio::test]
async fn free_plan_has_no_expiration() {
    let (server, client) = client().await;
    mount_user(
        &server,
        json(r#"{"success":true,"data":{"email":"me@example.com","plan":0,"premium_expires_at":"2025-11-01T00:00:00"}}"#),
    )
    .await;

    assert_eq!(client.user().await.expect("user").expiration, None);
}

#[tokio::test]
async fn blank_api_key_is_not_configured() {
    let server = MockServer::start().await;
    let mut config = common::config();
    config.api_key = String::new();
    let client = TorBox::new(&config, common::http()).with_base_url(&server.uri());

    let error = client.user().await.expect_err("blank key");

    assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn error_envelopes_report_code_and_detail() {
    for (body, message) in [
        (
            r#"{"success":false,"error":"BAD_TOKEN","detail":"Invalid token."}"#,
            "BAD_TOKEN: Invalid token.",
        ),
        (
            r#"{"success":false,"error":"BAD_TOKEN","detail":"BAD_TOKEN: Invalid."}"#,
            "BAD_TOKEN: Invalid.",
        ),
        (
            r#"{"success":false,"error":"BAD_TOKEN","detail":null}"#,
            "BAD_TOKEN",
        ),
    ] {
        let (server, client) = client().await;
        mount_user(&server, ResponseTemplate::new(401).set_body_string(body)).await;

        let error = client.user().await.expect_err("error envelope");

        assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
        assert_eq!(error.to_string(), message);
    }
}

#[tokio::test]
async fn missing_items_are_not_found() {
    let (server, client) = client().await;
    mount_user(
        &server,
        json(r#"{"success":false,"error":"ITEM_NOT_FOUND","detail":"Gone."}"#),
    )
    .await;

    let error = client.user().await.expect_err("missing");

    assert!(error.is_not_found(), "{error:?}");
}

#[tokio::test]
async fn transient_errors_are_retried() {
    let (server, client) = client().await;
    Mock::given(path("/user/me"))
        .respond_with(json(
            r#"{"success":false,"error":"DATABASE_ERROR","detail":"busy"}"#,
        ))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    mount_user(
        &server,
        json(r#"{"success":true,"data":{"email":"me@example.com"}}"#),
    )
    .await;

    let user = client.user().await.expect("recovered");

    assert_eq!(user.username.as_deref(), Some("me@example.com"));
    assert_eq!(server.received_requests().await.expect("recorded").len(), 3);
}

#[tokio::test]
async fn unsuccessful_envelopes_without_code_give_up_after_the_retries() {
    let (server, client) = client().await;
    mount_user(
        &server,
        json(r#"{"success":false,"detail":"Something broke."}"#),
    )
    .await;

    let error = client.user().await.expect_err("still failing");

    assert_eq!(error.to_string(), "UNKNOWN_ERROR: Something broke.");
    assert_eq!(server.received_requests().await.expect("recorded").len(), 3);
}

#[tokio::test]
async fn limit_errors_are_rate_limits_that_block_the_host() {
    for body in [
        r#"{"success":false,"error":"ACTIVE_LIMIT","detail":"Active limit reached"}"#,
        r#"{"success":false,"error":"rate_limit","detail":"Too fast"}"#,
        r#"{"success":true,"detail":"You hit the rate limit of 60 per minute"}"#,
        r#"{"success":false,"error":"PLAN","detail":"Only 10 per 1 hour"}"#,
    ] {
        let (server, client) = client().await;
        mount_user(&server, json(body)).await;

        let error = client.user().await.expect_err("rate limited");

        assert_eq!(
            error.retry_after(),
            Some(Duration::from_secs(120)),
            "{body}"
        );
        let second = client.user().await.expect_err("cooling down");
        assert!(second.to_string().contains("cooldown"), "{second}");
        assert_eq!(server.received_requests().await.expect("recorded").len(), 1);
    }
}

#[tokio::test]
async fn slow_down_messages_are_rate_limits() {
    let coordinator = Arc::new(RateLimitCoordinator::new());
    let server = MockServer::start().await;
    let http = ProviderHttp::new(Duration::from_secs(5), Arc::clone(&coordinator))
        .expect("client builds")
        .with_retry_policy(FAST_RETRY);
    let client = TorBox::new(&common::config(), http).with_base_url(&server.uri());
    mount_user(
        &server,
        ResponseTemplate::new(503).set_body_string("slow_down"),
    )
    .await;

    let error = client.user().await.expect_err("rate limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(120)));
    let host = server.address().ip().to_string();
    assert!(coordinator.remaining_cooldown(&host) > Duration::from_secs(110));
}

#[tokio::test]
async fn too_many_requests_is_a_rate_limit() {
    let (server, client) = client().await;
    mount_user(
        &server,
        ResponseTemplate::new(429)
            .insert_header("Retry-After", "30")
            .set_body_string(r#"{"success":false,"error":"RATE_LIMIT"}"#),
    )
    .await;

    let error = client.user().await.expect_err("rate limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(30)));
}

const TORRENT: &str = r#"{
    "id": 12345, "hash": "abcdef", "name": "Some.Release", "size": 3000,
    "created_at": "2024-05-20T14:12:52Z", "updated_at": "2024-05-21T10:00:00Z",
    "download_state": "downloading", "seeds": 7, "progress": 0.426,
    "download_speed": 1048576, "download_present": false,
    "files": [
        {"id": 0, "name": "Some.Release/Some.Release.mkv", "size": 2900},
        {"id": 1, "name": "Some.Release/Subs/en.srt", "size": 100},
        {"id": 2, "name": "loose.nfo", "size": 0}
    ]
}"#;

const QUEUED: &str =
    r#"{"id": 77, "hash": "fedcba", "name": "Waiting", "created_at": "2024-05-22T08:00:00Z"}"#;

async fn mount_mylist(server: &MockServer, id: Option<&str>, body: String) {
    let mut mock = Mock::given(method("GET"))
        .and(path("/torrents/mylist"))
        .and(query_param("bypass_cache", "true"))
        .and(query_param("limit", "1000"));
    if let Some(id) = id {
        mock = mock.and(query_param("id", id));
    }
    mock.respond_with(json(&body)).mount(server).await;
}

async fn mount_queued(server: &MockServer, id: Option<&str>, body: String) {
    let mut mock = Mock::given(method("GET"))
        .and(path("/queued/getqueued"))
        .and(query_param("type", "torrent"))
        .and(query_param("bypass_cache", "true"))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "1000"));
    if let Some(id) = id {
        mock = mock.and(query_param("id", id));
    }
    mock.respond_with(json(&body)).mount(server).await;
}

fn data(value: &str) -> String {
    format!(r#"{{"success":true,"error":null,"detail":"ok","data":{value}}}"#)
}

fn ts(text: &str) -> Timestamp {
    text.parse().expect("timestamp")
}

#[tokio::test]
async fn listing_maps_torrents_and_queued_torrents() {
    let (server, client) = client().await;
    mount_mylist(&server, None, data(&format!("[{TORRENT}]"))).await;
    mount_queued(&server, None, data(&format!("[{QUEUED}]"))).await;

    let torrents = client.list_torrents().await.expect("list");

    assert_eq!(torrents.len(), 2);
    let torrent = &torrents[0];
    assert_eq!(torrent.id, "12345");
    assert_eq!(torrent.filename, "Some.Release");
    assert_eq!(torrent.original_filename.as_deref(), Some("Some.Release"));
    assert_eq!(torrent.hash, "abcdef");
    assert_eq!((torrent.bytes, torrent.original_bytes), (3000, 3000));
    assert_eq!(torrent.host.as_deref(), Some("False"));
    assert_eq!(torrent.progress, 42);
    assert_eq!(torrent.status.as_deref(), Some("downloading"));
    assert_eq!(torrent.added, Some(ts("2024-05-20T14:12:52Z")));
    assert_eq!(torrent.ended, Some(ts("2024-05-21T10:00:00Z")));
    assert_eq!((torrent.speed, torrent.seeders), (Some(1_048_576), Some(7)));
    let files = torrent.files.as_ref().expect("files");
    let paths: Vec<(i64, &str, i64, bool)> = files
        .iter()
        .map(|file| (file.id, file.path.as_str(), file.bytes, file.selected))
        .collect();
    assert_eq!(
        paths,
        [
            (0, "Some.Release.mkv", 2900, true),
            (1, "Subs/en.srt", 100, true),
            (2, "", 0, true)
        ]
    );

    let queued = &torrents[1];
    assert_eq!(queued.id, "77");
    assert_eq!(queued.hash, "fedcba");
    assert_eq!(queued.status.as_deref(), Some("queued"));
    assert_eq!(queued.progress, 0);
    assert_eq!(queued.host.as_deref(), Some("False"));
    assert_eq!(queued.files, Some(Vec::new()));
    assert_eq!(queued.added, Some(ts("2024-05-22T08:00:00Z")));
    assert_eq!(queued.ended, queued.added);
}

#[tokio::test]
async fn listing_skips_missing_lists() {
    let (server, client) = client().await;
    mount_mylist(&server, None, data("null")).await;
    mount_queued(&server, None, data("[]")).await;

    assert_eq!(client.list_torrents().await.expect("list"), Vec::new());
}

#[tokio::test]
async fn lookup_prefers_current_torrents() {
    let (server, client) = client().await;
    mount_mylist(&server, Some("12345"), data(TORRENT)).await;

    let torrent = client
        .torrent("12345")
        .await
        .expect("lookup")
        .expect("found");

    assert_eq!(torrent.status.as_deref(), Some("downloading"));
    assert_eq!(server.received_requests().await.expect("recorded").len(), 1);
}

#[tokio::test]
async fn lookup_falls_back_to_queued_torrents() {
    for current in [
        data("null"),
        r#"{"success":false,"error":"ITEM_NOT_FOUND","detail":"Not found."}"#.to_owned(),
    ] {
        let (server, client) = client().await;
        mount_mylist(&server, Some("77"), current).await;
        mount_queued(&server, Some("77"), data(QUEUED)).await;

        let torrent = client.torrent("77").await.expect("lookup").expect("found");

        assert_eq!(torrent.status.as_deref(), Some("queued"));
    }
}

#[tokio::test]
async fn lookup_of_an_unknown_torrent_finds_nothing() {
    let (server, client) = client().await;
    mount_mylist(&server, Some("5"), data("null")).await;
    mount_queued(&server, Some("5"), data("[]")).await;

    assert_eq!(client.torrent("5").await.expect("lookup"), None);
    let error = client.torrent("abcdef").await.expect_err("not a number");
    assert_eq!(error.to_string(), "Invalid TorBox torrent id: abcdef");
}

async fn mount_seed_setting(server: &MockServer, seed: Option<i64>) {
    let settings = seed.map_or_else(
        || "null".to_owned(),
        |seed| format!(r#"{{"seed_torrents":{seed}}}"#),
    );
    Mock::given(method("GET"))
        .and(path("/user/me"))
        .and(query_param("settings", "true"))
        .respond_with(json(&data(&format!(
            r#"{{"email":"me@example.com","settings":{settings}}}"#
        ))))
        .mount(server)
        .await;
}

fn multipart_field(body: &str, name: &str) -> Option<String> {
    let marker = format!("name=\"{name}\"\r\n\r\n");
    let start = body.find(&marker)? + marker.len();
    let end = body[start..].find("\r\n")?;
    Some(body[start..start + end].to_owned())
}

#[tokio::test]
async fn add_magnet_creates_an_unqueued_torrent_with_the_seed_setting() {
    let (server, client) = client().await;
    mount_seed_setting(&server, Some(5)).await;
    Mock::given(method("POST"))
        .and(path("/torrents/createtorrent"))
        .and(header("authorization", "Bearer test-api-key"))
        .respond_with(json(&data(r#"{"torrent_id":4242,"hash":"abc"}"#)))
        .expect(1)
        .mount(&server)
        .await;

    let id = client
        .add_magnet("magnet:?xt=urn:btih:abc")
        .await
        .expect("added");

    assert_eq!(id, "4242");
    let requests = server.received_requests().await.expect("recorded");
    let body = String::from_utf8_lossy(&requests[1].body).into_owned();
    assert_eq!(
        multipart_field(&body, "magnet").as_deref(),
        Some("magnet:?xt=urn:btih:abc")
    );
    assert_eq!(multipart_field(&body, "seed").as_deref(), Some("5"));
    assert_eq!(
        multipart_field(&body, "allow_zip").as_deref(),
        Some("false")
    );
    assert_eq!(
        multipart_field(&body, "as_queued").as_deref(),
        Some("false")
    );
    assert_eq!(
        multipart_field(&body, "add_only_if_cached").as_deref(),
        Some("false")
    );
    assert_eq!(multipart_field(&body, "name"), None);
}

#[tokio::test]
async fn add_torrent_file_uploads_the_file_and_honours_zip_preference() {
    let server = MockServer::start().await;
    let mut config = common::config();
    config.prefer_zipped_downloads = true;
    let client = TorBox::new(&config, common::http()).with_base_url(&server.uri());
    mount_seed_setting(&server, None).await;
    Mock::given(method("POST"))
        .and(path("/torrents/createtorrent"))
        .respond_with(json(&data(r#"{"torrent_id":7}"#)))
        .mount(&server)
        .await;

    let id = client
        .add_torrent_file(b"torrent body")
        .await
        .expect("added");

    assert_eq!(id, "7");
    let requests = server.received_requests().await.expect("recorded");
    let body = String::from_utf8_lossy(&requests[1].body).into_owned();
    assert!(
        body.contains(r#"name="file"; filename="torrent.torrent""#),
        "{body}"
    );
    assert!(body.contains("application/x-bittorrent"), "{body}");
    assert!(body.contains("torrent body"), "{body}");
    assert_eq!(multipart_field(&body, "seed").as_deref(), Some("3"));
    assert_eq!(multipart_field(&body, "allow_zip").as_deref(), Some("true"));
}

#[tokio::test]
async fn add_without_a_torrent_id_fails() {
    let (server, client) = client().await;
    mount_seed_setting(&server, Some(1)).await;
    Mock::given(path("/torrents/createtorrent"))
        .respond_with(json(&data("{}")))
        .mount(&server)
        .await;

    let error = client
        .add_magnet("magnet:?xt=urn:btih:abc")
        .await
        .expect_err("no id");

    assert_eq!(error.to_string(), "TorBox API did not return torrent ID.");
}

#[tokio::test]
async fn add_on_active_limit_is_rate_limited_and_never_queued() {
    for limit in [
        r#"{"success":false,"error":"active_limit","detail":"Active limit reached"}"#,
        r#"{"success":false,"error":"UNAUTHORIZED","detail":"slow_down"}"#,
    ] {
        let (server, client) = client().await;
        mount_seed_setting(&server, Some(5)).await;
        Mock::given(path("/torrents/createtorrent"))
            .respond_with(json(limit))
            .expect(1)
            .mount(&server)
            .await;

        let magnet = client
            .add_magnet("magnet:?xt=urn:btih:abc")
            .await
            .expect_err("limited");
        assert_eq!(
            magnet.retry_after(),
            Some(Duration::from_secs(120)),
            "{limit}"
        );
        let requests = server.received_requests().await.expect("recorded");
        let body = String::from_utf8_lossy(&requests[1].body).into_owned();
        assert_eq!(
            multipart_field(&body, "as_queued").as_deref(),
            Some("false")
        );
    }
}

#[tokio::test]
async fn add_file_on_active_limit_is_rate_limited() {
    let (server, client) = client().await;
    mount_seed_setting(&server, Some(5)).await;
    Mock::given(path("/torrents/createtorrent"))
        .respond_with(json(
            r#"{"success":false,"error":"ACTIVE_LIMIT","detail":"Active limit reached"}"#,
        ))
        .mount(&server)
        .await;

    let error = client
        .add_torrent_file(b"torrent")
        .await
        .expect_err("limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(120)));
    assert_eq!(error.to_string(), "ACTIVE_LIMIT: Active limit reached");
}

#[tokio::test]
async fn add_keeps_the_cooldown_announced_by_too_many_requests() {
    let (server, client) = client().await;
    mount_seed_setting(&server, Some(5)).await;
    Mock::given(path("/torrents/createtorrent"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .mount(&server)
        .await;

    let error = client
        .add_magnet("magnet:?xt=urn:btih:abc")
        .await
        .expect_err("limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(3600)));
}

#[tokio::test]
async fn delete_controls_started_and_queued_torrents_differently() {
    for (current, queued, endpoint, body) in [
        (
            data(TORRENT),
            data("null"),
            "/torrents/controltorrent",
            r#"{"operation":"delete","torrent_id":12345}"#,
        ),
        (
            data("null"),
            data(QUEUED),
            "/torrents/controlqueued",
            r#"{"operation":"delete","queued_id":12345}"#,
        ),
        (
            data("null"),
            data("[]"),
            "/torrents/controltorrent",
            r#"{"operation":"delete","torrent_id":12345}"#,
        ),
    ] {
        let (server, client) = client().await;
        mount_mylist(&server, Some("12345"), current).await;
        mount_queued(&server, Some("12345"), queued).await;
        Mock::given(method("POST"))
            .and(path(endpoint))
            .and(body_json_string(body))
            .respond_with(json(r#"{"success":true,"detail":"Deleted."}"#))
            .expect(1)
            .mount(&server)
            .await;

        client.delete_torrent("12345").await.expect("deleted");
    }
}

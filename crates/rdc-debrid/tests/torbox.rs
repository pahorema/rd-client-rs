//! TorBox client against a mock API.

mod common;

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use rdc_debrid::{
    DebridError, DebridProvider, ProviderHttp, RateLimitCoordinator, RemoteTorrent, RetryPolicy,
    TorBox,
};
use rdc_model::{DownloadClient, DownloadInfo, Provider, RemoteFile, Torrent, TorrentStatus};
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
async fn speed_test_uses_the_closest_server() {
    let (server, client) = client().await;
    Mock::given(method("GET"))
        .and(path("/speedtest"))
        .respond_with(json(
            r#"{"success":true,"data":[
                {"url":"https://far.example/dld/100MB.bin","closest":false},
                {"url":"https://near.example/dld/100MB.bin","closest":true}]}"#,
        ))
        .mount(&server)
        .await;

    assert_eq!(
        client.speed_test_url().await.expect("url").as_deref(),
        Some("https://near.example/dld/100MB.bin")
    );
}

#[tokio::test]
async fn speed_test_without_a_closest_server_uses_the_first() {
    let (server, client) = client().await;
    Mock::given(path("/speedtest"))
        .respond_with(json(
            r#"{"success":true,"data":[{"url":"https://a.example/f"},{"url":"https://b.example/f"}]}"#,
        ))
        .mount(&server)
        .await;

    assert_eq!(
        client.speed_test_url().await.expect("url").as_deref(),
        Some("https://a.example/f")
    );
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
async fn api_keys_are_sent_without_surrounding_whitespace() {
    let server = MockServer::start().await;
    mount_user(
        &server,
        json(r#"{"success":true,"data":{"email":"me@example.com"}}"#),
    )
    .await;
    let mut config = common::config();
    config.api_key = format!(" {}\n", common::API_KEY);
    let client = TorBox::new(&config, common::http()).with_base_url(&server.uri());

    client.user().await.expect("user");

    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer test-api-key")
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

/// Answers listing calls to `endpoint` sent with `offset` with `items`.
async fn mount_page(server: &MockServer, endpoint: &str, offset: &str, items: &[String]) {
    Mock::given(method("GET"))
        .and(path(endpoint))
        .and(query_param("offset", offset))
        .and(query_param("limit", "1000"))
        .respond_with(json(&data(&format!("[{}]", items.join(",")))))
        .expect(1)
        .mount(server)
        .await;
}

fn queued(id: usize) -> String {
    format!(r#"{{"id": {id}, "hash": "h{id}", "name": "Waiting {id}"}}"#)
}

#[tokio::test]
async fn listing_reads_pages_until_one_that_is_not_full() {
    let (server, client) = client().await;
    let full: Vec<String> = (0..1000)
        .map(|id| TORRENT.replace("12345", &id.to_string()))
        .collect();
    mount_page(&server, "/torrents/mylist", "0", &full).await;
    mount_page(&server, "/torrents/mylist", "1000", &[TORRENT.to_owned()]).await;
    let full: Vec<String> = (0..1000).map(queued).collect();
    mount_page(&server, "/queued/getqueued", "0", &full).await;
    mount_page(&server, "/queued/getqueued", "1000", &[]).await;

    let torrents = client.list_torrents().await.expect("list");

    assert_eq!(torrents.len(), 2001);
    assert_eq!(torrents[1000].id, "12345");
    assert_eq!(torrents[2000].status.as_deref(), Some("queued"));
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

fn listed(state: &str, present: bool) -> RemoteTorrent {
    RemoteTorrent {
        id: "12345".to_owned(),
        filename: "Some.Release".to_owned(),
        original_filename: Some("Some.Release".to_owned()),
        host: Some(if present { "True" } else { "False" }.to_owned()),
        status: Some(state.to_owned()),
        files: Some(Vec::new()),
        ..RemoteTorrent::default()
    }
}

fn sent_torrent(previous: Option<TorrentStatus>) -> Torrent {
    Torrent {
        rd_id: Some("12345".to_owned()),
        rd_status: previous,
        ..Torrent::default()
    }
}

async fn refreshed(remote: &RemoteTorrent, previous: Option<TorrentStatus>) -> Torrent {
    let (server, client) = client().await;
    let mut torrent = sent_torrent(previous);
    client
        .refresh(&mut torrent, Some(remote))
        .await
        .expect("refreshed");
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
    torrent
}

#[tokio::test]
async fn refresh_maps_download_states() {
    for (state, expected) in [
        ("allocating", TorrentStatus::Processing),
        ("metaDL", TorrentStatus::Processing),
        ("queued", TorrentStatus::Processing),
        ("Queued (position 3)", TorrentStatus::Processing),
        ("completed", TorrentStatus::Downloading),
        ("processing", TorrentStatus::Downloading),
        ("paused", TorrentStatus::Downloading),
        ("stalled (no seeds)", TorrentStatus::Downloading),
        ("downloading", TorrentStatus::Downloading),
        ("checkingResumeData", TorrentStatus::Downloading),
        ("waiting", TorrentStatus::Downloading),
        ("uploading", TorrentStatus::Uploading),
        ("cached", TorrentStatus::Finished),
        ("error", TorrentStatus::Error),
        (
            "failed (Aborted, cannot be completed)",
            TorrentStatus::Error,
        ),
    ] {
        let torrent = refreshed(&listed(state, false), Some(TorrentStatus::Downloading)).await;

        assert_eq!(torrent.rd_status, Some(expected), "{state}");
        assert_eq!(torrent.rd_status_raw.as_deref(), Some(state));
    }
}

#[tokio::test]
async fn refresh_treats_present_downloads_as_finished() {
    let torrent = refreshed(&listed("downloading", true), None).await;

    assert_eq!(torrent.rd_status, Some(TorrentStatus::Finished));
    assert_eq!(torrent.rd_host.as_deref(), Some("True"));
}

#[tokio::test]
async fn refresh_keeps_the_previous_status_for_unmapped_states() {
    for (previous, expected) in [
        (Some(TorrentStatus::Downloading), TorrentStatus::Downloading),
        (Some(TorrentStatus::Finished), TorrentStatus::Finished),
        (Some(TorrentStatus::Queued), TorrentStatus::Processing),
        (None, TorrentStatus::Processing),
    ] {
        let torrent = refreshed(&listed("moving", false), previous).await;

        assert_eq!(torrent.rd_status, Some(expected), "{previous:?}");
    }
}

#[tokio::test]
async fn refresh_copies_the_torrent_fields() {
    let remote = RemoteTorrent {
        bytes: 0,
        original_bytes: 3000,
        progress: 42,
        added: Some(ts("2024-05-20T14:12:52Z")),
        ended: Some(ts("2024-05-21T10:00:00Z")),
        speed: Some(1024),
        seeders: Some(7),
        files: Some(vec![RemoteFile {
            id: 1,
            path: "Some.Release.mkv".to_owned(),
            bytes: 3000,
            selected: true,
            download_link: None,
        }]),
        ..listed("downloading", false)
    };

    let torrent = refreshed(&remote, None).await;

    assert_eq!(torrent.client_kind, Some(Provider::TorBox));
    assert_eq!(torrent.rd_name.as_deref(), Some("Some.Release"));
    assert_eq!(torrent.rd_size, Some(3000));
    assert_eq!(torrent.rd_progress, Some(42));
    assert_eq!(torrent.rd_split, Some(0));
    assert_eq!(torrent.rd_added, remote.added);
    assert_eq!(torrent.rd_ended, remote.ended);
    assert_eq!(
        (torrent.rd_speed, torrent.rd_seeders),
        (Some(1024), Some(7))
    );
    assert_eq!(torrent.files(), remote.files.clone().expect("files"));
}

#[tokio::test]
async fn refresh_fetches_the_torrent_when_not_listed() {
    let (server, client) = client().await;
    mount_mylist(&server, Some("12345"), data(TORRENT)).await;
    let mut torrent = sent_torrent(None);

    client.refresh(&mut torrent, None).await.expect("refreshed");

    assert_eq!(torrent.rd_status, Some(TorrentStatus::Downloading));
    assert_eq!(torrent.rd_size, Some(3000));
    assert_eq!(torrent.files().len(), 3);
}

#[tokio::test]
async fn refresh_marks_a_vanished_torrent_deleted() {
    let (server, client) = client().await;
    mount_mylist(&server, Some("12345"), data("null")).await;
    mount_queued(&server, Some("12345"), data("[]")).await;
    let mut torrent = sent_torrent(Some(TorrentStatus::Downloading));

    client.refresh(&mut torrent, None).await.expect("refreshed");

    assert_eq!(torrent.rd_status_raw.as_deref(), Some("deleted"));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Downloading));
}

#[tokio::test]
async fn refresh_without_a_provider_id_does_nothing() {
    let (server, client) = client().await;
    let mut torrent = Torrent::default();

    client.refresh(&mut torrent, None).await.expect("refreshed");

    assert_eq!(torrent, Torrent::default());
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

fn with_files(files: &[(i64, &str, i64)]) -> Torrent {
    let mut torrent = Torrent {
        rd_name: Some("TestTorrent".to_owned()),
        ..sent_torrent(None)
    };
    let files: Vec<RemoteFile> = files
        .iter()
        .map(|&(id, path, bytes)| RemoteFile {
            id,
            path: path.to_owned(),
            bytes,
            selected: true,
            download_link: None,
        })
        .collect();
    torrent.set_files(&files);
    torrent
}

fn zipping_client() -> TorBox {
    let mut config = common::config();
    config.prefer_zipped_downloads = true;
    TorBox::new(&config, common::http())
}

fn links(infos: &[DownloadInfo]) -> Vec<(Option<&str>, &str)> {
    infos
        .iter()
        .map(|info| (info.file_name.as_deref(), info.restricted_link.as_str()))
        .collect()
}

#[tokio::test]
async fn download_infos_link_every_file_without_zip_preference() {
    let (_server, client) = client().await;
    let torrent = with_files(&[(1, "a.mkv", 10), (2, "Subs/b.srt", 1)]);

    let infos = client
        .download_infos(&torrent)
        .expect("infos")
        .expect("ready");

    assert_eq!(
        links(&infos),
        [
            (Some("a.mkv"), "https://torbox.app/fakedl/12345/1"),
            (Some("b.srt"), "https://torbox.app/fakedl/12345/2"),
        ]
    );
}

#[test]
fn download_infos_use_one_zip_when_every_file_is_wanted() {
    let mut torrent = with_files(&[(1, "a.mkv", 10), (2, "b.srt", 1)]);
    torrent.download_client = DownloadClient::Aria2c;

    let infos = zipping_client()
        .download_infos(&torrent)
        .expect("infos")
        .expect("ready");

    assert_eq!(
        links(&infos),
        [(
            Some("TestTorrent.zip"),
            "https://torbox.app/fakedl/12345/zip"
        )]
    );
}

#[test]
fn download_infos_link_files_when_some_are_filtered_or_symlinked() {
    let mut filtered = with_files(&[(1, "a.mkv", 10), (2, "b.srt", 1)]);
    filtered.exclude_regex = Some(r"\.srt$".to_owned());
    let mut symlinked = with_files(&[(1, "a.mkv", 10)]);
    symlinked.download_client = DownloadClient::Symlink;

    let filtered = zipping_client()
        .download_infos(&filtered)
        .expect("infos")
        .expect("ready");
    let symlinked = zipping_client()
        .download_infos(&symlinked)
        .expect("infos")
        .expect("ready");

    assert_eq!(
        links(&filtered),
        [(Some("a.mkv"), "https://torbox.app/fakedl/12345/1")]
    );
    assert_eq!(
        links(&symlinked),
        [(Some("a.mkv"), "https://torbox.app/fakedl/12345/1")]
    );
}

#[tokio::test]
async fn download_infos_wait_for_a_provider_id_and_report_all_excluded() {
    let (_server, client) = client().await;
    let mut excluded = with_files(&[(1, "a.mkv", 10)]);
    excluded.include_regex = Some("nothing".to_owned());

    assert_eq!(
        client.download_infos(&Torrent::default()).expect("infos"),
        None
    );
    assert_eq!(
        client.download_infos(&excluded).expect("infos"),
        Some(Vec::new())
    );
}

async fn mount_requestdl(server: &MockServer, file_id: &str, zip: &str) {
    Mock::given(method("GET"))
        .and(path("/torrents/requestdl"))
        .and(header("authorization", "Bearer test-api-key"))
        .and(query_param("token", "test-api-key"))
        .and(query_param("torrent_id", "12345"))
        .and(query_param("file_id", file_id))
        .and(query_param("zip_link", zip))
        .and(query_param("redirect", "false"))
        .and(query_param("append_name", "false"))
        .respond_with(json(&data(r#""https://cdn.torbox.app/file""#)))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn unrestrict_requests_single_files_and_zips() {
    let (server, client) = client().await;
    mount_requestdl(&server, "6789", "false").await;
    mount_requestdl(&server, "0", "true").await;

    let file = client
        .unrestrict_link("https://torbox.app/fakedl/12345/6789")
        .await;
    let zip = client
        .unrestrict_link("https://torbox.app/fakedl/12345/zip")
        .await;

    assert_eq!(file.expect("file link"), "https://cdn.torbox.app/file");
    assert_eq!(zip.expect("zip link"), "https://cdn.torbox.app/file");
}

#[tokio::test]
async fn unrestrict_accepts_any_link_with_ids_in_the_last_segments() {
    let (server, client) = client().await;
    Mock::given(path("/torrents/requestdl"))
        .and(query_param("torrent_id", "123"))
        .and(query_param("file_id", "456"))
        .and(query_param("zip_link", "false"))
        .respond_with(json(&data(r#""https://cdn/x""#)))
        .expect(1)
        .mount(&server)
        .await;

    assert_eq!(
        client
            .unrestrict_link("https://torbox.app/d/123/456")
            .await
            .expect("link"),
        "https://cdn/x"
    );
}

#[tokio::test]
async fn unrestrict_rejects_malformed_links() {
    let (server, client) = client().await;
    for (link, message) in [
        ("", "Link cannot be null or empty"),
        (
            "https://torbox.app/fakedl/12345",
            "Invalid link format: https://torbox.app/fakedl/12345",
        ),
        (
            "https://torbox.app/fakedl/abc/1",
            "Invalid torrent ID in link segment 4: abc",
        ),
        (
            "https://torbox.app/fakedl/1/x9",
            "Invalid file ID in link segment 5: x9",
        ),
    ] {
        let error = client.unrestrict_link(link).await.expect_err("malformed");

        assert_eq!(error.to_string(), message);
    }
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn unrestrict_without_a_link_fails() {
    let (server, client) = client().await;
    Mock::given(path("/torrents/requestdl"))
        .respond_with(json(&data("null")))
        .mount(&server)
        .await;

    let error = client
        .unrestrict_link("https://torbox.app/fakedl/1/2")
        .await
        .expect_err("no link");

    assert!(
        error
            .to_string()
            .starts_with("Unrestrict returned an invalid download"),
        "{error}"
    );
}

#[tokio::test]
async fn available_files_come_from_the_cache_check() {
    let (server, client) = client().await;
    Mock::given(method("GET"))
        .and(path("/torrents/checkcached"))
        .and(query_param("hash", "abc"))
        .and(query_param("format", "list"))
        .and(query_param("list_files", "true"))
        .respond_with(json(&data(
            r#"[{"name":"R","size":3,"hash":"abc","files":[{"name":"R/a.mkv","size":2},{"name":"R/b.srt","size":1}]}]"#,
        )))
        .mount(&server)
        .await;

    let files = client.available_files("abc").await.expect("files");

    let files: Vec<(&str, i64)> = files
        .iter()
        .map(|file| (file.filename.as_str(), file.filesize))
        .collect();
    assert_eq!(files, [("R/a.mkv", 2), ("R/b.srt", 1)]);
}

#[tokio::test]
async fn available_files_are_empty_when_not_cached() {
    for body in [data("[]"), data("{}"), data("null")] {
        let (server, client) = client().await;
        Mock::given(path("/torrents/checkcached"))
            .respond_with(json(&body))
            .mount(&server)
            .await;

        assert_eq!(
            client.available_files("abc").await.expect("files"),
            Vec::new()
        );
    }
}

#[tokio::test]
async fn provider_selects_once_the_files_are_known() {
    let (_server, client) = client().await;
    let provider: &dyn DebridProvider = &client;

    assert_eq!(provider.kind(), Provider::TorBox);
    assert_eq!(
        provider
            .select_files(&sent_torrent(None))
            .await
            .expect("select"),
        None
    );
    let torrent = with_files(&[(1, "a.mkv", 10), (2, "b.srt", 1)]);
    assert_eq!(
        provider.select_files(&torrent).await.expect("select"),
        Some(2)
    );
}

#[tokio::test]
async fn provider_delete_uses_the_provider_id() {
    let (server, client) = client().await;
    let provider: &dyn DebridProvider = &client;
    mount_mylist(&server, Some("12345"), data(TORRENT)).await;
    Mock::given(method("POST"))
        .and(path("/torrents/controltorrent"))
        .respond_with(json(r#"{"success":true}"#))
        .expect(1)
        .mount(&server)
        .await;

    provider
        .delete(&Torrent::default())
        .await
        .expect("nothing to delete");
    provider.delete(&sent_torrent(None)).await.expect("deleted");
}

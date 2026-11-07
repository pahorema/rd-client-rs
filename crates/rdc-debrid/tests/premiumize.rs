//! Premiumize client against a mock API.

mod common;

use std::time::Duration;

use jiff::Timestamp;
use rdc_debrid::{DebridError, DebridProvider, Premiumize, RemoteTorrent};
use rdc_model::{Download, DownloadInfo, Provider, Torrent, TorrentStatus};
use wiremock::matchers::{body_string, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::API_KEY;

async fn client() -> (MockServer, Premiumize) {
    let server = MockServer::start().await;
    let client = Premiumize::new(&common::config(), common::http()).with_base_url(&server.uri());
    (server, client)
}

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "application/json")
}

async fn mount_account(server: &MockServer, body: &str) {
    Mock::given(method("GET"))
        .and(path("/account/info"))
        .and(query_param("apikey", API_KEY))
        .respond_with(json(body))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn user_reports_customer_id_and_premium_end() {
    let (server, client) = client().await;
    mount_account(
        &server,
        r#"{"status":"success","customer_id":1234,"premium_until":1761955200}"#,
    )
    .await;

    let user = client.user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("1234"));
    assert_eq!(
        user.expiration,
        Some(Timestamp::from_second(1_761_955_200).expect("ts"))
    );
    assert!(!user.premium);
}

#[tokio::test]
async fn user_without_premium_has_no_expiration() {
    let (server, client) = client().await;
    mount_account(
        &server,
        r#"{"status":"success","customer_id":"42","premium_until":false}"#,
    )
    .await;

    let user = client.user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("42"));
    assert_eq!(user.expiration, None);
}

#[tokio::test]
async fn requests_authenticate_with_the_api_key_query_only() {
    let (server, client) = client().await;
    mount_account(&server, r#"{"status":"success","customer_id":1}"#).await;

    client.user().await.expect("user");

    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.query(), Some("apikey=test-api-key"));
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn blank_api_key_is_not_configured() {
    let server = MockServer::start().await;
    let mut config = common::config();
    config.api_key = "  ".to_owned();
    let client = Premiumize::new(&config, common::http()).with_base_url(&server.uri());

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
async fn error_status_becomes_a_provider_error_with_its_message() {
    let (server, client) = client().await;
    mount_account(&server, r#"{"status":"error","message":"Not logged in."}"#).await;

    let error = client.user().await.expect_err("error status");

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert_eq!(error.to_string(), "Not logged in.");
}

#[tokio::test]
async fn rate_limit_messages_become_rate_limited_for_two_minutes() {
    for message in [
        "rate limit exceeded",
        "You've made too many API requests too quickly.",
        "Your fair-use points, booster points, or active-job count is exhausted.",
        "This account's usage limit for this service has been reached.",
        "The target service is unreachable right now. Retry after a delay.",
    ] {
        let (server, client) = client().await;
        let body = serde_json::json!({"status": "error", "message": message}).to_string();
        mount_account(&server, &body).await;

        let error = client.user().await.expect_err("rate limited");

        assert_eq!(
            error.retry_after(),
            Some(Duration::from_secs(120)),
            "{message}"
        );
        assert_eq!(error.to_string(), message);
    }
}

#[tokio::test]
async fn rate_limit_codes_become_rate_limited() {
    for code in [
        "slow_down",
        "rate_limit_reached",
        "account_limit_reached",
        "service_limit_reached",
        "service_down",
        "semi_permanent_error",
    ] {
        let (server, client) = client().await;
        let body = serde_json::json!({"status": "error", "code": code, "message": "retry later"})
            .to_string();
        mount_account(&server, &body).await;

        let error = client.user().await.expect_err("rate limited");

        assert_eq!(
            error.retry_after(),
            Some(Duration::from_secs(120)),
            "{code}"
        );
    }
}

#[tokio::test]
async fn empty_and_unreadable_responses_are_invalid() {
    for response in [
        ResponseTemplate::new(204),
        ResponseTemplate::new(200).set_body_string("<html>maintenance</html>"),
        ResponseTemplate::new(502).set_body_string("Bad gateway"),
    ] {
        let server = MockServer::start().await;
        let client =
            Premiumize::new(&common::config(), common::http()).with_base_url(&server.uri());
        Mock::given(path("/account/info"))
            .respond_with(response)
            .mount(&server)
            .await;

        let error = client.user().await.expect_err("invalid");

        assert!(
            matches!(error, DebridError::InvalidResponse(_)),
            "{error:?}"
        );
    }
}

#[tokio::test]
async fn timeouts_name_the_cause_but_not_the_api_key() {
    let server = MockServer::start().await;
    Mock::given(path("/account/info"))
        .respond_with(
            json(r#"{"status":"success","customer_id":1}"#).set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let client = Premiumize::new(
        &common::config(),
        common::http_with_timeout(Duration::from_millis(50)),
    )
    .with_base_url(&server.uri());

    let error = client.user().await.expect_err("timed out");

    let text = error.to_string();
    assert!(matches!(error, DebridError::Transport(_)), "{error:?}");
    assert!(text.contains("timed out"), "{text}");
    assert!(!text.contains(API_KEY), "{text}");
}

#[tokio::test]
async fn refused_connections_name_the_cause_but_not_the_api_key() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    let address = listener.local_addr().expect("address");
    drop(listener);
    let client = Premiumize::new(&common::config(), common::http())
        .with_base_url(&format!("http://{address}/"));

    let error = client.user().await.expect_err("refused");

    let text = error.to_string();
    assert!(matches!(error, DebridError::Transport(_)), "{error:?}");
    assert!(text.contains("refused"), "{text}");
    assert!(!text.contains(API_KEY), "{text}");
}

#[tokio::test]
async fn transfers_are_listed_without_sizes_or_files() {
    let (server, client) = client().await;
    Mock::given(method("POST"))
        .and(path("/transfer/list"))
        .and(query_param("apikey", API_KEY))
        .respond_with(json(
            r#"{"status":"success","transfers":[
                {"id":"t1","name":"Some.Release","message":"Downloading","status":"running",
                 "progress":0.257,"folder_id":"f1","file_id":null,"src":"magnet:?xt=urn:btih:abc"},
                {"id":"t2","name":"Other","status":"finished","progress":null,"folder_id":null}
            ]}"#,
        ))
        .mount(&server)
        .await;

    let torrents = client.list_torrents().await.expect("list");

    assert_eq!(torrents.len(), 2);
    let first = &torrents[0];
    assert_eq!(first.id, "t1");
    assert_eq!(first.filename, "Some.Release");
    assert_eq!(first.original_filename.as_deref(), Some("Some.Release"));
    assert_eq!(first.hash, "magnet:?xt=urn:btih:abc");
    assert_eq!(first.progress, 25);
    assert_eq!(first.status.as_deref(), Some("running"));
    assert_eq!(first.message.as_deref(), Some("Downloading"));
    assert_eq!((first.bytes, first.original_bytes), (0, 0));
    assert_eq!(first.files, Some(Vec::new()));
    assert_eq!(first.links, Some(vec!["f1".to_owned()]));
    assert_eq!((first.speed, first.seeders), (Some(0), Some(0)));
    assert_eq!(torrents[1].progress, 100, "missing progress counts as done");
    assert_eq!(torrents[1].links, Some(Vec::new()));
}

#[tokio::test]
async fn add_magnet_posts_the_source_form_and_returns_the_transfer_id() {
    let (server, client) = client().await;
    Mock::given(method("POST"))
        .and(path("/transfer/create"))
        .respond_with(json(
            r#"{"status":"success","id":"magnet-transfer-id","name":"test"}"#,
        ))
        .mount(&server)
        .await;

    let id = client
        .add_magnet("magnet:?xt=urn:btih:test")
        .await
        .expect("added");

    assert_eq!(id, "magnet-transfer-id");
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests[0].url.query(), Some("apikey=test-api-key"));
    assert!(!requests[0].headers.contains_key("authorization"));
    assert_eq!(
        String::from_utf8_lossy(&requests[0].body),
        "src=magnet%3A%3Fxt%3Durn%3Abtih%3Atest&folder_id="
    );
}

#[tokio::test]
async fn add_torrent_file_uploads_a_multipart_torrent() {
    let (server, client) = client().await;
    Mock::given(method("POST"))
        .and(path("/transfer/create"))
        .respond_with(json(
            r#"{"status":"success","id":"torrent-transfer-id","name":"test"}"#,
        ))
        .mount(&server)
        .await;

    let id = client
        .add_torrent_file(b"torrent body")
        .await
        .expect("added");

    assert_eq!(id, "torrent-transfer-id");
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests[0].url.query(), Some("apikey=test-api-key"));
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(
        body.contains(r#"name="file"; filename="1.torrent""#),
        "{body}"
    );
    assert!(body.contains("application/x-bittorrent"), "{body}");
    assert!(body.contains("torrent body"), "{body}");
    assert!(body.contains(r#"name="folder_id""#), "{body}");
}

#[tokio::test]
async fn add_without_a_transfer_id_fails() {
    let (server, client) = client().await;
    Mock::given(path("/transfer/create"))
        .respond_with(json(r#"{"status":"success","id":""}"#))
        .mount(&server)
        .await;

    let error = client
        .add_magnet("magnet:?xt=urn:btih:test")
        .await
        .expect_err("no id");

    assert_eq!(error.to_string(), "Unable to add magnet link");
}

#[tokio::test]
async fn delete_posts_the_transfer_id() {
    let (server, client) = client().await;
    Mock::given(method("POST"))
        .and(path("/transfer/delete"))
        .and(query_param("apikey", API_KEY))
        .and(body_string("id=t1"))
        .respond_with(json(r#"{"status":"success"}"#))
        .expect(1)
        .mount(&server)
        .await;

    client.delete_transfer("t1").await.expect("deleted");
}

async fn mount_transfers(server: &MockServer, body: &str) {
    Mock::given(method("POST"))
        .and(path("/transfer/list"))
        .respond_with(json(body))
        .mount(server)
        .await;
}

fn sent_torrent(rd_id: &str) -> Torrent {
    Torrent {
        rd_id: Some(rd_id.to_owned()),
        ..Torrent::default()
    }
}

#[tokio::test]
async fn refresh_maps_transfer_statuses() {
    for (raw, expected) in [
        ("waiting", TorrentStatus::Processing),
        ("queued", TorrentStatus::Processing),
        ("running", TorrentStatus::Downloading),
        ("seeding", TorrentStatus::Finished),
        ("finished", TorrentStatus::Finished),
        ("error", TorrentStatus::Error),
        ("timeout", TorrentStatus::Error),
        ("banned", TorrentStatus::Error),
    ] {
        let (server, client) = client().await;
        let body = serde_json::json!({"status": "success", "transfers": [
            {"id": "t1", "name": "Some.Release", "status": raw, "progress": 0.5}
        ]})
        .to_string();
        mount_transfers(&server, &body).await;
        let mut torrent = sent_torrent("t1");

        client.refresh(&mut torrent, None).await.expect("refreshed");

        assert_eq!(torrent.rd_status, Some(expected), "{raw}");
        assert_eq!(torrent.rd_status_raw.as_deref(), Some(raw));
    }
}

#[tokio::test]
async fn refresh_copies_the_listed_transfer() {
    let (server, client) = client().await;
    let mut torrent = Torrent {
        rd_name: Some("Old".to_owned()),
        rd_size: Some(123),
        rd_files: Some(
            r#"[{"Id":1,"Path":"a.mkv","Bytes":1,"Selected":true,"DownloadLink":null}]"#.to_owned(),
        ),
        ..sent_torrent("t1")
    };
    let remote = RemoteTorrent {
        id: "t1".to_owned(),
        filename: "Some.Release".to_owned(),
        progress: 42,
        status: Some("running".to_owned()),
        files: Some(Vec::new()),
        speed: Some(0),
        seeders: Some(0),
        ..RemoteTorrent::default()
    };

    client
        .refresh(&mut torrent, Some(&remote))
        .await
        .expect("refreshed");

    assert_eq!(torrent.client_kind, Some(Provider::Premiumize));
    assert_eq!(torrent.rd_name.as_deref(), Some("Some.Release"));
    assert_eq!(torrent.rd_size, Some(123), "sizes are never reported");
    assert_eq!(torrent.rd_files.as_deref(), Some("[]"));
    assert_eq!(torrent.rd_progress, Some(42));
    assert_eq!((torrent.rd_speed, torrent.rd_seeders), (Some(0), Some(0)));
    assert_eq!((torrent.rd_added, torrent.rd_ended), (None, None));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Downloading));
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn refresh_marks_a_vanished_transfer_deleted() {
    let (server, client) = client().await;
    mount_transfers(&server, r#"{"status":"success","transfers":[]}"#).await;
    let mut torrent = Torrent {
        rd_status: Some(TorrentStatus::Downloading),
        ..sent_torrent("t1")
    };

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

async fn mount_folder(server: &MockServer, id: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/folder/list"))
        .and(query_param("id", id))
        .and(query_param("includebreadcrumbs", "true"))
        .and(query_param("apikey", API_KEY))
        .respond_with(json(&body.to_string()))
        .expect(1)
        .mount(server)
        .await;
}

fn infos(list: &[DownloadInfo]) -> Vec<(Option<&str>, &str)> {
    list.iter()
        .map(|info| (info.file_name.as_deref(), info.restricted_link.as_str()))
        .collect()
}

#[tokio::test]
async fn download_infos_walk_the_transfer_folder() {
    let (server, client) = client().await;
    mount_transfers(
        &server,
        r#"{"status":"success","transfers":[{"id":"t1","name":"R","status":"finished","folder_id":"f1"}]}"#,
    )
    .await;
    mount_folder(&server, "f1", serde_json::json!({"status": "success", "content": [
        {"id": "a", "name": "movie.mkv", "size": 900, "type": "file", "link": "https://dl/movie.mkv"},
        {"id": "b", "name": "Subs", "type": "folder"},
        {"id": "c", "name": "no-link.nfo", "size": 1, "type": "file", "link": ""},
        {"id": "d", "name": "odd", "type": "unknown"}
    ]}))
    .await;
    mount_folder(
        &server,
        "b",
        serde_json::json!({"status": "success", "content": [
            {"id": "e", "name": "en.srt", "size": 5, "type": "file", "link": "https://dl/en.srt"}
        ]}),
    )
    .await;

    let found = client
        .download_infos(&sent_torrent("t1"))
        .await
        .expect("infos");

    let found = found.expect("ready");
    assert_eq!(
        infos(&found),
        [
            (Some("movie.mkv"), "https://dl/movie.mkv"),
            (Some("en.srt"), "https://dl/en.srt")
        ]
    );
}

#[tokio::test]
async fn download_infos_filter_files_by_size_and_folders_by_name() {
    let (server, client) = client().await;
    mount_transfers(
        &server,
        r#"{"status":"success","transfers":[{"id":"t1","name":"R","status":"finished","folder_id":"f1"}]}"#,
    )
    .await;
    mount_folder(&server, "f1", serde_json::json!({"status": "success", "content": [
        {"id": "a", "name": "movie.mkv", "size": 5_000_000, "type": "file", "link": "https://dl/movie.mkv"},
        {"id": "s", "name": "sample.mkv", "size": 10, "type": "file", "link": "https://dl/sample.mkv"},
        {"id": "x", "name": "Extras", "type": "folder"}
    ]}))
    .await;
    let torrent = Torrent {
        download_min_size: 1,
        exclude_regex: Some("Extras".to_owned()),
        ..sent_torrent("t1")
    };

    let found = client
        .download_infos(&torrent)
        .await
        .expect("infos")
        .expect("ready");

    assert_eq!(infos(&found), [(Some("movie.mkv"), "https://dl/movie.mkv")]);
}

#[tokio::test]
async fn download_infos_add_the_single_file_of_a_transfer() {
    let (server, client) = client().await;
    mount_transfers(
        &server,
        r#"{"status":"success","transfers":[{"id":"t1","name":"R","status":"finished","folder_id":"","file_id":"i1"}]}"#,
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/item/details"))
        .and(query_param("id", "i1"))
        .respond_with(json(
            r#"{"id":"i1","name":"tiny.txt","size":1,"type":"file","link":"https://dl/tiny.txt"}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    let torrent = Torrent {
        download_min_size: 1,
        ..sent_torrent("t1")
    };

    let found = client
        .download_infos(&torrent)
        .await
        .expect("infos")
        .expect("ready");

    assert_eq!(infos(&found), [(Some("tiny.txt"), "https://dl/tiny.txt")]);
}

#[tokio::test]
async fn download_infos_wait_for_a_provider_id_and_need_the_transfer() {
    let (server, client) = client().await;
    mount_transfers(&server, r#"{"status":"success","transfers":[]}"#).await;

    assert_eq!(
        client
            .download_infos(&Torrent::default())
            .await
            .expect("infos"),
        None
    );
    let error = client
        .download_infos(&sent_torrent("t1"))
        .await
        .expect_err("missing");
    assert!(error.is_not_found(), "{error:?}");
}

#[tokio::test]
async fn provider_selects_once_and_passes_links_through() {
    let (server, client) = client().await;
    let provider: &dyn DebridProvider = &client;
    let torrent = sent_torrent("t1");

    assert_eq!(provider.kind(), Provider::Premiumize);
    assert_eq!(
        provider.select_files(&torrent).await.expect("select"),
        Some(1)
    );
    assert_eq!(
        provider
            .unrestrict(&torrent, "https://dl/movie.mkv")
            .await
            .expect("link"),
        "https://dl/movie.mkv"
    );
    assert_eq!(
        provider.available_files("abc").await.expect("files"),
        Vec::new()
    );
    provider
        .delete(&Torrent::default())
        .await
        .expect("nothing to delete");
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn provider_file_name_comes_from_the_download() {
    let (_server, client) = client().await;
    let provider: &dyn DebridProvider = &client;
    let named = Download {
        file_name: Some("movie.mkv".to_owned()),
        ..Download::default()
    };

    assert_eq!(provider.file_name(&named).await.expect("name"), "movie.mkv");
}

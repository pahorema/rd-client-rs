//! Debrid-Link client against a mock API.

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use rdc_debrid::{
    DebridError, DebridLink, DebridProvider, ProviderConfig, ProviderHttp, RateLimitCoordinator,
    RemoteTorrent, RetryPolicy,
};
use rdc_model::{Download, DownloadInfo, Provider, RemoteFile, Torrent, TorrentStatus};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(api_key: &str) -> ProviderConfig {
    ProviderConfig {
        api_key: api_key.to_owned(),
        api_hostname: None,
        timeout: Duration::from_secs(5),
        prefer_zipped_downloads: false,
    }
}

fn client_with_key(server: &MockServer, api_key: &str) -> DebridLink {
    let http = ProviderHttp::new(
        Duration::from_secs(5),
        Arc::new(RateLimitCoordinator::new()),
    )
    .expect("client builds")
    .with_retry_policy(RetryPolicy {
        max_retries: 0,
        base_delay: Duration::from_millis(1),
    });
    DebridLink::new(&config(api_key), http).with_base_url(&server.uri())
}

fn client(server: &MockServer) -> DebridLink {
    client_with_key(server, "test-key")
}

fn ok(value: &Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "success": true, "value": value }))
}

#[tokio::test]
async fn user_reports_name_and_expiry_from_remaining_premium_seconds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/account/infos"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ok(&json!({ "username": "alice", "premiumLeft": 86_400 })))
        .expect(1)
        .mount(&server)
        .await;

    let before = Timestamp::now();
    let user = client(&server).user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("alice"));
    assert!(!user.premium);
    let expiration = user.expiration.expect("premium account expires");
    let remaining = expiration.duration_since(before);
    assert!(
        remaining >= SignedDuration::from_hours(24)
            && remaining < SignedDuration::from_hours(24) + SignedDuration::from_secs(60),
        "{remaining:?}"
    );
}

#[tokio::test]
async fn user_without_premium_time_has_no_expiry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/account/infos"))
        .respond_with(ok(&json!({ "username": "bob", "premiumLeft": 0 })))
        .mount(&server)
        .await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("bob"));
    assert_eq!(user.expiration, None);
}

#[tokio::test]
async fn blank_api_key_is_not_configured_and_sends_nothing() {
    let server = MockServer::start().await;

    let error = client_with_key(&server, "  ")
        .user()
        .await
        .expect_err("no key");

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
    Mock::given(method("GET"))
        .and(path("/account/infos"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ok(&json!({ "username": "alice" })))
        .expect(1)
        .mount(&server)
        .await;

    client_with_key(&server, " test-key\n")
        .user()
        .await
        .expect("user");
}

#[tokio::test]
async fn failed_envelope_reports_the_api_error_code() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({ "success": false, "error": "badToken" })),
        )
        .mount(&server)
        .await;

    let error = client(&server).user().await.expect_err("bad token");

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert!(error.to_string().contains("badToken"), "{error}");
}

#[tokio::test]
async fn error_status_without_envelope_reports_the_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad gateway</html>"))
        .mount(&server)
        .await;

    let error = client(&server).user().await.expect_err("bad gateway");

    assert!(error.to_string().contains("502"), "{error}");
}

#[tokio::test]
async fn unreadable_success_body_is_an_invalid_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let error = client(&server).user().await.expect_err("garbage");

    assert!(
        matches!(error, DebridError::InvalidResponse(_)),
        "{error:?}"
    );
}

fn seedbox_torrent(id: &str) -> Value {
    json!({
        "id": id,
        "name": "Some.Release",
        "hashString": "3641ad15ad451518f558d5ce5b76be4a618911c1",
        "serverId": "srv-7",
        "peersConnected": 12,
        "status": 4,
        "totalSize": 8162,
        "created": 1_700_000_000,
        "downloadPercent": 42,
        "downloadSpeed": 2048,
        "uploadSpeed": 99,
        "files": [
            { "id": "a", "name": "Some.Release/a.mkv", "size": 8120, "downloadUrl": "https://dl.example/a.mkv" },
            { "id": "b", "name": "Some.Release/b.nfo", "size": 42, "downloadUrl": "https://dl.example/b.nfo" }
        ]
    })
}

fn list_page(page: &str, items: &Value) -> Mock {
    Mock::given(method("GET"))
        .and(path("/seedbox/list"))
        .and(query_param("page", page))
        .and(query_param("perPage", "50"))
        .respond_with(ok(items))
}

/// A full listing page of torrents `<prefix>0` to `<prefix>49` whose
/// pagination names `next` as the following page.
fn full_page(page: &str, prefix: &str, next: i64) -> Mock {
    let items: Vec<Value> = (0..50)
        .map(|index| seedbox_torrent(&format!("{prefix}{index}")))
        .collect();
    Mock::given(method("GET"))
        .and(path("/seedbox/list"))
        .and(query_param("page", page))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "value": items,
            "pagination": { "page": page.parse::<i64>().expect("page"), "pages": 2, "next": next }
        })))
}

#[tokio::test]
async fn listing_reads_full_pages_until_a_short_one() {
    let server = MockServer::start().await;
    full_page("0", "a", 1).expect(1).mount(&server).await;
    list_page("1", &json!([seedbox_torrent("t1"), seedbox_torrent("t2")]))
        .expect(1)
        .mount(&server)
        .await;
    list_page("2", &json!([])).expect(0).mount(&server).await;

    let torrents = client(&server).list_torrents().await.expect("list");

    assert_eq!(torrents.len(), 52);
    assert_eq!(torrents[51].id, "t2");
}

#[tokio::test]
async fn listing_stops_on_the_last_page_the_api_reports() {
    let server = MockServer::start().await;
    full_page("0", "a", 1).expect(1).mount(&server).await;
    full_page("1", "b", -1).expect(1).mount(&server).await;
    full_page("2", "b", -1).expect(0).mount(&server).await;

    let torrents = client(&server).list_torrents().await.expect("list");

    assert_eq!(torrents.len(), 100);
}

#[tokio::test]
async fn listing_maps_torrent_fields_and_reports_the_download_speed() {
    let server = MockServer::start().await;
    list_page("0", &json!([seedbox_torrent("t1")]))
        .mount(&server)
        .await;
    list_page("1", &json!([])).mount(&server).await;

    let torrents = client(&server).list_torrents().await.expect("list");

    let torrent = &torrents[0];
    assert_eq!(torrent.filename, "Some.Release");
    assert_eq!(torrent.original_filename.as_deref(), Some("Some.Release"));
    assert_eq!(torrent.hash, "3641ad15ad451518f558d5ce5b76be4a618911c1");
    assert_eq!(torrent.bytes, 8162);
    assert_eq!(torrent.host.as_deref(), Some("srv-7"));
    assert_eq!(torrent.progress, 42);
    assert_eq!(torrent.status.as_deref(), Some("4"));
    assert_eq!(
        torrent.added,
        Some(Timestamp::from_second(1_700_000_000).expect("valid"))
    );
    assert_eq!(torrent.ended, None);
    assert_eq!(torrent.speed, Some(2048));
    assert_eq!(torrent.seeders, Some(12));
    assert_eq!(
        torrent.links.as_deref(),
        Some(
            [
                "https://dl.example/a.mkv".to_owned(),
                "https://dl.example/b.nfo".to_owned()
            ]
            .as_slice()
        )
    );
    let files = torrent.files.as_deref().expect("files");
    assert_eq!(files.len(), 2);
    assert_eq!(
        (files[1].id, files[1].path.as_str(), files[1].bytes),
        (1, "Some.Release/b.nfo", 42)
    );
    assert!(files.iter().all(|file| file.selected));
    assert_eq!(
        files[0].download_link.as_deref(),
        Some("https://dl.example/a.mkv")
    );
}

#[tokio::test]
async fn adding_a_magnet_posts_the_url_and_returns_the_new_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/seedbox/add"))
        .and(body_string_contains(
            "url=magnet%3A%3Fxt%3Durn%3Abtih%3Aabc",
        ))
        .and(body_string_contains("wait=false"))
        .and(body_string_contains("async=false"))
        .respond_with(ok(&seedbox_torrent("t9")))
        .expect(1)
        .mount(&server)
        .await;

    let id = client(&server)
        .add_magnet("magnet:?xt=urn:btih:abc")
        .await
        .expect("added");

    assert_eq!(id, "t9");
}

#[tokio::test]
async fn adding_a_torrent_file_uploads_it_as_a_multipart_part() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/seedbox/add"))
        .and(body_string_contains(
            "name=\"file\"; filename=\"1.torrent\"",
        ))
        .and(body_string_contains("application/x-bittorrent"))
        .and(body_string_contains("d8:announce"))
        .respond_with(ok(&json!({ "id": "t10" })))
        .expect(1)
        .mount(&server)
        .await;

    let id = client(&server)
        .add_torrent_file(b"d8:announce0:e")
        .await
        .expect("added");

    assert_eq!(id, "t10");
}

#[tokio::test]
async fn adding_without_an_id_in_the_answer_returns_an_empty_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ok(&Value::Null))
        .mount(&server)
        .await;

    let id = client(&server).add_magnet("magnet:?").await.expect("added");

    assert_eq!(id, "");
}

#[tokio::test]
async fn adding_maps_slow_down_errors_to_a_two_minute_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "success": false, "error": "slow_down" })),
        )
        .mount(&server)
        .await;

    let error = client(&server)
        .add_magnet("magnet:?")
        .await
        .expect_err("rate limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(120)));
    assert!(error.to_string().contains("slow_down"), "{error}");
}

#[tokio::test]
async fn adding_keeps_other_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "success": false, "error": "maxTorrent" })),
        )
        .mount(&server)
        .await;

    let error = client(&server)
        .add_magnet("magnet:?")
        .await
        .expect_err("limit");

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
}

#[tokio::test]
async fn deleting_removes_the_torrent_by_id() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/seedbox/t1/remove"))
        .respond_with(ok(&json!(["t1"])))
        .expect(1)
        .mount(&server)
        .await;
    let torrent = Torrent {
        rd_id: Some("t1".to_owned()),
        ..Torrent::default()
    };

    client(&server).delete(&torrent).await.expect("deleted");
}

#[tokio::test]
async fn deleting_a_torrent_without_provider_id_sends_nothing() {
    let server = MockServer::start().await;

    client(&server)
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

fn info_by_id(id: &str, items: &Value) -> Mock {
    Mock::given(method("GET"))
        .and(path("/seedbox/list"))
        .and(query_param("ids", id))
        .respond_with(ok(items))
}

fn with_id(id: &str) -> Torrent {
    Torrent {
        rd_id: Some(id.to_owned()),
        ..Torrent::default()
    }
}

#[tokio::test]
async fn refresh_without_a_listing_entry_looks_the_torrent_up() {
    let server = MockServer::start().await;
    info_by_id("t1", &json!([seedbox_torrent("t1")]))
        .expect(1)
        .mount(&server)
        .await;
    let mut torrent = with_id("t1");

    client(&server)
        .refresh(&mut torrent, None)
        .await
        .expect("refreshed");

    assert_eq!(torrent.client_kind, Some(Provider::DebridLink));
    assert_eq!(torrent.rd_name.as_deref(), Some("Some.Release"));
    assert_eq!(torrent.rd_size, Some(8162));
    assert_eq!(torrent.rd_host.as_deref(), Some("srv-7"));
    assert_eq!(torrent.rd_split, Some(0));
    assert_eq!(torrent.rd_progress, Some(42));
    assert_eq!(
        torrent.rd_added,
        Some(Timestamp::from_second(1_700_000_000).expect("valid"))
    );
    assert_eq!(torrent.rd_ended, None);
    assert_eq!(torrent.rd_speed, Some(2048));
    assert_eq!(torrent.rd_seeders, Some(12));
    assert_eq!(torrent.rd_status_raw.as_deref(), Some("4"));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Downloading));
    let files = torrent.files();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "Some.Release/a.mkv");
}

#[tokio::test]
async fn refresh_uses_the_listing_entry_without_a_request() {
    let server = MockServer::start().await;
    let remote = RemoteTorrent {
        id: "t1".to_owned(),
        filename: "Listed.Name".to_owned(),
        status: Some("100".to_owned()),
        ..RemoteTorrent::default()
    };
    let mut torrent = with_id("t1");

    client(&server)
        .refresh(&mut torrent, Some(&remote))
        .await
        .expect("refreshed");

    assert_eq!(torrent.rd_name.as_deref(), Some("Listed.Name"));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Finished));
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn refresh_maps_every_seedbox_status() {
    let server = MockServer::start().await;
    let cases = [
        ("0", TorrentStatus::Error),
        ("1", TorrentStatus::Processing),
        ("2", TorrentStatus::Processing),
        ("3", TorrentStatus::Processing),
        ("4", TorrentStatus::Downloading),
        ("5", TorrentStatus::Finished),
        ("6", TorrentStatus::Finished),
        ("100", TorrentStatus::Finished),
        ("42", TorrentStatus::Error),
    ];
    for (raw, expected) in cases {
        let remote = RemoteTorrent {
            status: Some(raw.to_owned()),
            ..RemoteTorrent::default()
        };
        let mut torrent = with_id("t1");

        client(&server)
            .refresh(&mut torrent, Some(&remote))
            .await
            .expect("refreshed");

        assert_eq!(torrent.rd_status, Some(expected), "status {raw}");
        assert_eq!(torrent.rd_status_raw.as_deref(), Some(raw));
    }
}

#[tokio::test]
async fn refresh_marks_a_torrent_missing_from_the_seedbox_as_deleted() {
    let server = MockServer::start().await;
    info_by_id("gone", &json!([])).mount(&server).await;
    let mut torrent = Torrent {
        rd_status: Some(TorrentStatus::Downloading),
        ..with_id("gone")
    };

    client(&server)
        .refresh(&mut torrent, None)
        .await
        .expect("refreshed");

    assert_eq!(torrent.rd_status_raw.as_deref(), Some("deleted"));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Downloading));
}

#[tokio::test]
async fn refresh_without_provider_id_leaves_the_torrent_untouched() {
    let server = MockServer::start().await;
    let mut torrent = Torrent::default();

    client(&server)
        .refresh(&mut torrent, None)
        .await
        .expect("refreshed");

    assert_eq!(torrent, Torrent::default());
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn download_infos_list_linked_files_that_pass_the_filter() {
    let server = MockServer::start().await;
    let mut listed = seedbox_torrent("t1");
    listed["files"]
        .as_array_mut()
        .expect("files")
        .push(json!({ "name": "Some.Release/c.mkv", "size": 9000, "downloadUrl": null }));
    info_by_id("t1", &json!([listed])).mount(&server).await;
    let torrent = Torrent {
        exclude_regex: Some(r"\.nfo$".to_owned()),
        ..with_id("t1")
    };

    let infos = client(&server)
        .download_infos(&torrent)
        .await
        .expect("infos")
        .expect("ready");

    assert_eq!(
        infos,
        [DownloadInfo {
            file_name: Some("a.mkv".to_owned()),
            restricted_link: "https://dl.example/a.mkv".to_owned(),
        }]
    );
}

#[tokio::test]
async fn download_infos_are_not_ready_while_the_torrent_has_no_files() {
    let server = MockServer::start().await;
    let mut listed = seedbox_torrent("t1");
    listed.as_object_mut().expect("object").remove("files");
    info_by_id("t1", &json!([listed])).mount(&server).await;

    let infos = client(&server)
        .download_infos(&with_id("t1"))
        .await
        .expect("infos");

    assert_eq!(infos, None);
}

#[tokio::test]
async fn download_infos_of_a_missing_torrent_are_not_found() {
    let server = MockServer::start().await;
    info_by_id("gone", &json!([])).mount(&server).await;

    let error = client(&server)
        .download_infos(&with_id("gone"))
        .await
        .expect_err("missing");

    assert!(error.is_not_found(), "{error:?}");
}

fn provider(server: &MockServer) -> Box<dyn DebridProvider> {
    Box::new(client(server))
}

fn torrent_with_files(paths: &[&str]) -> Torrent {
    let files: Vec<RemoteFile> = paths
        .iter()
        .zip(0..)
        .map(|(path, id)| RemoteFile {
            id,
            path: (*path).to_owned(),
            bytes: 10,
            selected: true,
            download_link: None,
        })
        .collect();
    let mut torrent = Torrent {
        rd_name: Some("Some.Release".to_owned()),
        ..with_id("t1")
    };
    torrent.set_files(&files);
    torrent
}

fn named_download(file_name: Option<&str>) -> Download {
    Download {
        path: "https://dl.example/a.mkv".to_owned(),
        file_name: file_name.map(str::to_owned),
        ..Download::default()
    }
}

#[tokio::test]
async fn provider_selects_every_file_without_a_request() {
    let server = MockServer::start().await;
    let provider = provider(&server);

    let selected = provider
        .select_files(&torrent_with_files(&["a.mkv", "b.nfo"]))
        .await
        .expect("selected");

    assert_eq!(provider.kind(), Provider::DebridLink);
    assert_eq!(selected, Some(2));
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn provider_waits_with_the_selection_until_files_are_known() {
    let server = MockServer::start().await;

    let selected = provider(&server)
        .select_files(&torrent_with_files(&[]))
        .await
        .expect("selection");

    assert_eq!(selected, None);
}

#[tokio::test]
async fn provider_links_are_already_unrestricted_and_nothing_is_cached() {
    let server = MockServer::start().await;
    let provider = provider(&server);

    let link = provider
        .unrestrict(&with_id("t1"), "https://dl.example/a.mkv")
        .await
        .expect("unrestricted");
    let cached = provider
        .available_files("3641ad15ad451518f558d5ce5b76be4a618911c1")
        .await
        .expect("cache");

    assert_eq!(link, "https://dl.example/a.mkv");
    assert_eq!(cached.len(), 0);
}

#[tokio::test]
async fn provider_file_name_is_the_stored_download_name() {
    let server = MockServer::start().await;

    let name = provider(&server)
        .file_name(&named_download(Some("a.mkv")))
        .await
        .expect("name");

    assert_eq!(name, "a.mkv");
}

#[tokio::test]
async fn symlink_path_of_a_single_file_torrent_is_the_file_name() {
    let server = MockServer::start().await;

    let path = provider(&server)
        .symlink_path(
            &torrent_with_files(&["a.mkv"]),
            &named_download(Some("a.mkv")),
        )
        .expect("path");

    assert_eq!(path.as_deref(), Some("a.mkv"));
}

#[tokio::test]
async fn symlink_path_of_a_multi_file_torrent_is_inside_the_torrent_folder() {
    let server = MockServer::start().await;

    let path = provider(&server)
        .symlink_path(
            &torrent_with_files(&["a.mkv", "b.nfo"]),
            &named_download(Some("b.nfo")),
        )
        .expect("path");

    assert_eq!(
        path.as_deref().map(std::path::Path::new),
        Some(std::path::Path::new("Some.Release/b.nfo"))
    );
}

#[tokio::test]
async fn symlink_path_needs_torrent_and_file_names() {
    let server = MockServer::start().await;
    let provider = provider(&server);
    let unnamed_torrent = Torrent {
        rd_name: None,
        ..torrent_with_files(&["a.mkv"])
    };

    let without_file = provider
        .symlink_path(&torrent_with_files(&["a.mkv"]), &named_download(None))
        .expect("path");
    let without_torrent = provider
        .symlink_path(&unnamed_torrent, &named_download(Some("a.mkv")))
        .expect("path");

    assert_eq!((without_file, without_torrent), (None, None));
}

//! Real-Debrid client against a mocked REST API.

use std::sync::Arc;
use std::time::Duration;

use rdc_debrid::{
    DebridError, ProviderConfig, ProviderHttp, RateLimitCoordinator, RealDebrid, RemoteTorrent,
    RetryPolicy,
};
use rdc_model::{Provider, RemoteFile, Torrent, TorrentDownloadAction, TorrentStatus};
use serde_json::json;
use wiremock::matchers::{
    bearer_token, body_bytes, body_string, method, path, query_param, query_param_is_missing,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(api_key: &str, timeout: Duration) -> ProviderConfig {
    ProviderConfig {
        api_key: api_key.to_owned(),
        api_hostname: None,
        timeout,
        prefer_zipped_downloads: false,
    }
}

fn client_with(server: &MockServer, config: &ProviderConfig) -> RealDebrid {
    let http = ProviderHttp::new(
        Duration::from_secs(30),
        Arc::new(RateLimitCoordinator::new()),
    )
    .expect("client builds")
    .with_retry_policy(RetryPolicy {
        max_retries: 1,
        base_delay: Duration::from_millis(1),
    });
    RealDebrid::with_base_url(config, http, &format!("{}/rest/1.0/", server.uri()))
}

fn client(server: &MockServer) -> RealDebrid {
    client_with(server, &config("secret", Duration::from_secs(5)))
}

async fn mock_user(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/rest/1.0/user"))
        .and(bearer_token("secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(server)
        .await;
}

async fn user_error(server: &MockServer, response: ResponseTemplate) -> DebridError {
    Mock::given(method("GET"))
        .and(path("/rest/1.0/user"))
        .respond_with(response)
        .mount(server)
        .await;
    client(server).user().await.expect_err("request fails")
}

#[tokio::test]
async fn user_reports_the_expiration_while_premium_time_is_left() {
    let server = MockServer::start().await;
    mock_user(
        &server,
        json!({"id": 1, "username": "alice", "type": "premium", "premium": 3600,
               "expiration": "2030-01-02T03:04:05.000Z"}),
    )
    .await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("alice"));
    assert_eq!(
        user.expiration,
        Some("2030-01-02T03:04:05Z".parse().expect("timestamp"))
    );
    assert!(!user.premium);
}

#[tokio::test]
async fn user_without_premium_time_has_no_expiration() {
    let server = MockServer::start().await;
    mock_user(
        &server,
        json!({"username": "bob", "type": "free", "premium": 0,
               "expiration": "2020-01-02T03:04:05.000Z"}),
    )
    .await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.expiration, None);
}

#[tokio::test]
async fn blank_api_key_fails_without_sending_a_request() {
    let server = MockServer::start().await;
    let client = client_with(&server, &config(" ", Duration::from_secs(5)));

    let error = client.user().await.expect_err("no key");

    assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "The Real-Debrid API key is not set in the settings"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn unknown_resource_error_code_is_not_found() {
    let server = MockServer::start().await;
    let response = ResponseTemplate::new(404)
        .set_body_json(json!({"error": "unknown_ressource", "error_code": 7}));

    let error = user_error(&server, response).await;

    assert!(error.is_not_found(), "{error:?}");
    assert_eq!(error.to_string(), "Resource not found");
}

#[tokio::test]
async fn known_error_codes_are_reported_with_their_message() {
    let server = MockServer::start().await;
    let response =
        ResponseTemplate::new(401).set_body_json(json!({"error": "bad_token", "error_code": 8}));

    let error = user_error(&server, response).await;

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert_eq!(error.to_string(), "Bad token");
}

#[tokio::test]
async fn unknown_error_codes_keep_the_provider_text() {
    let server = MockServer::start().await;
    let response = ResponseTemplate::new(400)
        .set_body_json(json!({"error": "something_new", "error_code": 99}));

    let error = user_error(&server, response).await;

    assert_eq!(error.to_string(), "something_new");
}

#[tokio::test]
async fn error_bodies_without_error_field_are_reported_verbatim() {
    let server = MockServer::start().await;
    let response = ResponseTemplate::new(503).set_body_string("<html>maintenance</html>");

    let error = user_error(&server, response).await;

    assert_eq!(error.to_string(), "<html>maintenance</html>");
}

#[tokio::test]
async fn empty_error_bodies_report_the_http_status() {
    let server = MockServer::start().await;

    let error = user_error(&server, ResponseTemplate::new(502)).await;

    assert!(error.to_string().contains("502"), "{error}");
}

#[tokio::test]
async fn requests_time_out_after_the_configured_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"username": "late"}))
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let client = client_with(&server, &config("secret", Duration::from_millis(100)));

    let error = client.user().await.expect_err("times out");

    assert!(matches!(error, DebridError::Transport(_)), "{error:?}");
}

async fn mock_server_time(server: &MockServer, time: &str) {
    Mock::given(method("GET"))
        .and(path("/rest/1.0/time/iso"))
        .respond_with(ResponseTemplate::new(200).set_body_string(time))
        .expect(1)
        .mount(server)
        .await;
}

fn listed(id: &str) -> serde_json::Value {
    json!({"id": id, "filename": format!("{id}.mkv"), "hash": format!("hash-{id}"),
           "bytes": 10, "original_bytes": 20, "host": "real-debrid.com", "split": 2000,
           "progress": 100, "status": "downloaded", "added": "2024-05-01T10:00:00.000Z",
           "ended": "2024-05-01T11:00:00.000Z", "links": [format!("https://real-debrid.com/d/{id}")]})
}

#[tokio::test]
async fn listing_requests_pages_until_an_empty_page() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T12:00:00+00:00").await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param_is_missing("offset"))
        .and(query_param("limit", "5000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([listed("A"), listed("B")])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param("offset", "5000"))
        .and(query_param("limit", "5000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([listed("C")])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param("offset", "10000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;

    let torrents = client(&server).list_torrents().await.expect("listing");

    let ids: Vec<&str> = torrents.iter().map(|torrent| torrent.id.as_str()).collect();
    assert_eq!(ids, ["A", "B", "C"]);
    let first = &torrents[0];
    assert_eq!(first.filename, "A.mkv");
    assert_eq!(first.hash, "hash-A");
    assert_eq!((first.bytes, first.original_bytes), (10, 20));
    assert_eq!(first.host.as_deref(), Some("real-debrid.com"));
    assert_eq!((first.split, first.progress), (2000, 100));
    assert_eq!(first.status.as_deref(), Some("downloaded"));
    assert_eq!(first.files, None);
    assert_eq!(
        first.links.as_deref(),
        Some(["https://real-debrid.com/d/A".to_owned()].as_slice())
    );
}

#[tokio::test]
async fn listing_without_content_is_empty() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T12:00:00+00:00").await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let torrents = client(&server).list_torrents().await.expect("listing");

    assert_eq!(torrents, Vec::new());
}

#[tokio::test]
async fn listing_corrects_times_with_the_server_offset_read_once() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T14:00:00+02:00").await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param_is_missing("offset"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([listed("A")])))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents"))
        .and(query_param("offset", "5000"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let client = client(&server);

    client.list_torrents().await.expect("first listing");
    let torrents = client.list_torrents().await.expect("second listing");

    assert_eq!(
        torrents[0].added,
        Some("2024-05-01T08:00:00Z".parse().expect("timestamp"))
    );
    assert_eq!(
        torrents[0].ended,
        Some("2024-05-01T09:00:00Z".parse().expect("timestamp"))
    );
}

#[tokio::test]
async fn unreadable_server_time_fails_the_listing() {
    let server = MockServer::start().await;
    mock_server_time(&server, "yesterday").await;

    let error = client(&server)
        .list_torrents()
        .await
        .expect_err("no offset");

    assert!(
        matches!(error, DebridError::InvalidResponse(_)),
        "{error:?}"
    );
}

#[tokio::test]
async fn adding_a_magnet_posts_it_and_returns_the_torrent_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/rest/1.0/torrents/addMagnet"))
        .and(bearer_token("secret"))
        .and(body_string("magnet=magnet%3A%3Fxt%3Durn%3Abtih%3Aabc"))
        .respond_with(ResponseTemplate::new(201).set_body_json(
            json!({"id": "GGZIOQZAZHDIE", "uri": "https://api.real-debrid.com/rest/1.0/torrents/info/GGZIOQZAZHDIE"}),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let id = client(&server)
        .add_magnet("magnet:?xt=urn:btih:abc")
        .await
        .expect("added");

    assert_eq!(id, "GGZIOQZAZHDIE");
}

#[tokio::test]
async fn adding_a_torrent_file_puts_the_raw_bytes() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/rest/1.0/torrents/addTorrent"))
        .and(body_bytes(b"d8:announce0:e".to_vec()))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "FILEID"})))
        .expect(1)
        .mount(&server)
        .await;

    let id = client(&server)
        .add_torrent_file(b"d8:announce0:e")
        .await
        .expect("added");

    assert_eq!(id, "FILEID");
}

#[tokio::test]
async fn too_many_requests_on_add_is_a_rate_limit_with_the_requested_delay() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/rest/1.0/torrents/addMagnet"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "30"))
        .expect(1)
        .mount(&server)
        .await;

    let error = client(&server)
        .add_magnet("magnet:?xt=urn:btih:abc")
        .await
        .expect_err("rate limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(30)));
}

#[tokio::test]
async fn rate_limit_text_on_add_is_a_two_minute_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/rest/1.0/torrents/addTorrent"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream rate limit exceeded"))
        .expect(1)
        .mount(&server)
        .await;

    let error = client(&server)
        .add_torrent_file(b"torrent")
        .await
        .expect_err("rate limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(120)));
    assert_eq!(error.to_string(), "upstream rate limit exceeded");
}

#[tokio::test]
async fn other_add_errors_are_not_rate_limits() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/rest/1.0/torrents/addMagnet"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": "torrent_file_invalid", "error_code": 30})),
        )
        .mount(&server)
        .await;

    let error = client(&server)
        .add_magnet("magnet:?xt=urn:btih:abc")
        .await
        .expect_err("rejected");

    assert_eq!(error.retry_after(), None);
    assert_eq!(error.to_string(), "Torrent file invalid");
}

fn torrent_with_id(id: &str) -> Torrent {
    Torrent {
        rd_id: Some(id.to_owned()),
        ..Torrent::default()
    }
}

fn info_body() -> serde_json::Value {
    json!({"id": "ID", "filename": "Name.mkv", "original_filename": "Original Name",
           "hash": "abc", "bytes": 0, "original_bytes": 300, "host": "real-debrid.com",
           "split": 2000, "progress": 37, "status": "downloading",
           "added": "2024-05-01T12:00:00.000Z", "ended": null, "speed": 1000, "seeders": 4,
           "files": [{"id": 1, "path": "/Name/a.mkv", "bytes": 200, "selected": 1},
                     {"id": 2, "path": "/Name/b.nfo", "bytes": 100, "selected": 0}],
           "links": []})
}

async fn mock_info(server: &MockServer, response: ResponseTemplate, expected_calls: u64) {
    Mock::given(method("GET"))
        .and(path("/rest/1.0/torrents/info/ID"))
        .respond_with(response)
        .expect(expected_calls)
        .mount(server)
        .await;
}

fn finished_remote() -> RemoteTorrent {
    RemoteTorrent {
        id: "ID".to_owned(),
        filename: "Listed.mkv".to_owned(),
        bytes: 50,
        original_bytes: 80,
        progress: 100,
        status: Some("downloaded".to_owned()),
        added: Some("2024-05-01T10:00:00Z".parse().expect("timestamp")),
        ended: Some("2024-05-01T11:00:00Z".parse().expect("timestamp")),
        files: Some(Vec::new()),
        ..RemoteTorrent::default()
    }
}

#[tokio::test]
async fn refreshing_a_torrent_without_provider_id_changes_nothing() {
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
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn refreshing_without_listing_entry_fetches_the_torrent_details() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T13:00:00+01:00").await;
    mock_info(
        &server,
        ResponseTemplate::new(200).set_body_json(info_body()),
        1,
    )
    .await;
    let mut torrent = torrent_with_id("ID");

    client(&server)
        .refresh(&mut torrent, None)
        .await
        .expect("refreshed");

    assert_eq!(torrent.client_kind, Some(Provider::RealDebrid));
    assert_eq!(torrent.rd_name.as_deref(), Some("Original Name"));
    assert_eq!(torrent.rd_size, Some(300));
    assert_eq!(torrent.rd_host.as_deref(), Some("real-debrid.com"));
    assert_eq!(
        (torrent.rd_split, torrent.rd_progress),
        (Some(2000), Some(37))
    );
    assert_eq!(
        (torrent.rd_speed, torrent.rd_seeders),
        (Some(1000), Some(4))
    );
    assert_eq!(torrent.rd_status_raw.as_deref(), Some("downloading"));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Downloading));
    assert_eq!(
        torrent.rd_added,
        Some("2024-05-01T11:00:00Z".parse().expect("timestamp"))
    );
    assert_eq!(torrent.rd_ended, None);
    assert_eq!(
        torrent.rd_files.as_deref(),
        Some(concat!(
            r#"[{"Id":1,"Path":"/Name/a.mkv","Bytes":200,"Selected":true,"DownloadLink":null},"#,
            r#"{"Id":2,"Path":"/Name/b.nfo","Bytes":100,"Selected":false,"DownloadLink":null}]"#
        ))
    );
}

#[tokio::test]
async fn refreshing_from_a_finished_listing_entry_sends_no_request() {
    let server = MockServer::start().await;
    let mut torrent = torrent_with_id("ID");
    torrent.rd_files = Some("[]".to_owned());
    torrent.rd_name = Some("Old".to_owned());

    client(&server)
        .refresh(&mut torrent, Some(&finished_remote()))
        .await
        .expect("refreshed");

    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    assert_eq!(torrent.rd_name.as_deref(), Some("Listed.mkv"));
    assert_eq!(torrent.rd_size, Some(50));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Finished));
    assert_eq!(torrent.rd_ended, finished_remote().ended);
    assert_eq!(
        torrent.rd_files.as_deref(),
        Some("[]"),
        "empty lists never clear files"
    );
}

#[tokio::test]
async fn refreshing_from_an_unfinished_listing_entry_fetches_the_details() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T12:00:00+00:00").await;
    mock_info(
        &server,
        ResponseTemplate::new(200).set_body_json(info_body()),
        1,
    )
    .await;
    let remote = RemoteTorrent {
        ended: None,
        ..finished_remote()
    };
    let mut torrent = torrent_with_id("ID");

    client(&server)
        .refresh(&mut torrent, Some(&remote))
        .await
        .expect("refreshed");

    assert_eq!(torrent.rd_status_raw.as_deref(), Some("downloading"));
}

#[tokio::test]
async fn refreshing_a_torrent_unknown_to_the_provider_marks_it_deleted() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T12:00:00+00:00").await;
    let response = ResponseTemplate::new(404)
        .set_body_json(json!({"error": "unknown_ressource", "error_code": 7}));
    mock_info(&server, response, 1).await;
    let mut torrent = torrent_with_id("ID");
    torrent.rd_status = Some(TorrentStatus::Downloading);

    client(&server)
        .refresh(&mut torrent, None)
        .await
        .expect("refreshed");

    assert_eq!(torrent.rd_status_raw.as_deref(), Some("deleted"));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Downloading));
    assert_eq!(torrent.client_kind, None);
}

#[tokio::test]
async fn other_refresh_errors_are_returned() {
    let server = MockServer::start().await;
    mock_server_time(&server, "2024-05-01T12:00:00+00:00").await;
    let response =
        ResponseTemplate::new(401).set_body_json(json!({"error": "bad_token", "error_code": 8}));
    mock_info(&server, response, 1).await;
    let mut torrent = torrent_with_id("ID");

    let error = client(&server)
        .refresh(&mut torrent, None)
        .await
        .expect_err("bad token");

    assert_eq!(error.to_string(), "Bad token");
    assert_eq!(torrent.rd_status_raw, None);
}

fn torrent_with_files(files: &[(i64, &str, i64)]) -> Torrent {
    let files: Vec<RemoteFile> = files
        .iter()
        .map(|&(id, path, bytes)| RemoteFile {
            id,
            path: path.to_owned(),
            bytes,
            ..RemoteFile::default()
        })
        .collect();
    let mut torrent = torrent_with_id("ID");
    torrent.client_kind = Some(Provider::RealDebrid);
    torrent.set_files(&files);
    torrent
}

async fn mock_select(server: &MockServer, files: &str, expected_calls: u64) {
    Mock::given(method("POST"))
        .and(path("/rest/1.0/torrents/selectFiles/ID"))
        .and(body_string(format!("files={}", files.replace(',', "%2C"))))
        .respond_with(ResponseTemplate::new(204))
        .expect(expected_calls)
        .mount(server)
        .await;
}

const MIB: i64 = 1024 * 1024;

#[tokio::test]
async fn selecting_files_sends_the_ids_of_downloadable_files() {
    let server = MockServer::start().await;
    mock_select(&server, "1,3", 1).await;
    let mut torrent = torrent_with_files(&[
        (1, "/Show/e01.mkv", 700 * MIB),
        (2, "/Show/sample.mkv", 5 * MIB),
        (3, "/Show/e02.mkv", 700 * MIB),
    ]);
    torrent.download_min_size = 10;

    let selected = client(&server)
        .select_files(&torrent)
        .await
        .expect("selected");

    assert_eq!(selected, Some(2));
}

#[tokio::test]
async fn manual_selection_keeps_matching_suffixes_regardless_of_size() {
    let server = MockServer::start().await;
    mock_select(&server, "2", 1).await;
    let mut torrent = torrent_with_files(&[(1, "/Show/e01.mkv", MIB), (2, "/Show/e02.mkv", MIB)]);
    torrent.download_action = TorrentDownloadAction::DownloadManual;
    torrent.download_manual_files = Some("e02.mkv".to_owned());
    torrent.download_min_size = 100;

    let selected = client(&server)
        .select_files(&torrent)
        .await
        .expect("selected");

    assert_eq!(selected, Some(1));
}

#[tokio::test]
async fn selecting_when_every_file_is_excluded_sends_nothing() {
    let server = MockServer::start().await;
    let mut torrent = torrent_with_files(&[(1, "/Show/e01.nfo", MIB)]);
    torrent.exclude_regex = Some(r"\.nfo$".to_owned());

    let selected = client(&server)
        .select_files(&torrent)
        .await
        .expect("selected");

    assert_eq!(selected, Some(0));
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn selecting_with_an_invalid_filter_fails() {
    let server = MockServer::start().await;
    let mut torrent = torrent_with_files(&[(1, "/Show/e01.mkv", MIB)]);
    torrent.include_regex = Some("(".to_owned());

    let error = client(&server)
        .select_files(&torrent)
        .await
        .expect_err("invalid filter");

    assert!(matches!(error, DebridError::Filter(_)), "{error:?}");
}

#[tokio::test]
async fn deleting_a_torrent_sends_a_delete_request() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/rest/1.0/torrents/delete/ID"))
        .and(bearer_token("secret"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    client(&server)
        .delete(&torrent_with_id("ID"))
        .await
        .expect("deleted");
}

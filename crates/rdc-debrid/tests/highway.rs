//! HIGHWAY client against a mock API.

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use rdc_debrid::{
    DebridError, Highway, ProviderConfig, ProviderHttp, RateLimitCoordinator, RemoteTorrent,
    RetryPolicy, is_spool_id,
};
use rdc_model::{Provider, Torrent, TorrentStatus};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ENDPOINT: &str = "/center-api/transfers.php";
const SPOOL_ID: &str = "3f9a1c2b4d5e6f708192a3b4c5d6e7f8";

fn config(api_key: &str) -> ProviderConfig {
    ProviderConfig {
        api_key: api_key.to_owned(),
        api_hostname: None,
        timeout: Duration::from_secs(5),
        prefer_zipped_downloads: false,
    }
}

fn client_with_key(server: &MockServer, api_key: &str) -> Highway {
    let http = ProviderHttp::new(
        Duration::from_secs(5),
        Arc::new(RateLimitCoordinator::new()),
    )
    .expect("client builds")
    .with_retry_policy(RetryPolicy {
        max_retries: 0,
        base_delay: Duration::from_millis(1),
    });
    Highway::new(&config(api_key), http).with_endpoint(&format!("{}{ENDPOINT}", server.uri()))
}

fn client(server: &MockServer) -> Highway {
    client_with_key(server, "test-token")
}

fn json_response(status: u16, body: &Value) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(body)
}

fn transfer(id: &str, state: &str) -> Value {
    json!({
        "id": id,
        "service": "torrent",
        "name": "Some.Release",
        "state": state,
        "state_detail": "boom",
        "progress": 62,
        "size": 1000,
        "downloaded": 400,
        "eta": 6,
        "seeder": 9,
        "content_path": "TORRENT/Some.Release",
        "download_url": "https://nl.dwld.link/dav/TORRENT/Some.Release/?download=zip",
        "files": [
            { "path": "Some.Release/Some.Release.mkv", "size": 990, "partial": false,
              "download_url": "https://nl.dwld.link/dav/TORRENT/Some.Release/Some.Release.mkv?sig=1" },
            { "path": "Some.Release/Some.Release.part", "size": 10, "partial": true, "download_url": null }
        ]
    })
}

async fn mount_listing(server: &MockServer, transfers: &Value) {
    Mock::given(method("GET"))
        .and(path(ENDPOINT))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(json_response(200, &json!({ "transfers": transfers })))
        .mount(server)
        .await;
}

#[test]
fn spool_ids_are_32_hex_characters() {
    assert!(is_spool_id(SPOOL_ID));
    assert!(!is_spool_id("4213"));
    assert!(!is_spool_id("3f9a1c2b4d5e6f708192a3b4c5d6e7fz"));
    assert!(!is_spool_id("3f9a1c2b4d5e6f708192a3b4c5d6e7f80"));
}

#[tokio::test]
async fn listing_returns_torrent_transfers_and_skips_other_services() {
    let server = MockServer::start().await;
    let usenet =
        json!({ "id": SPOOL_ID, "service": "usenet", "name": "Usenet.Release", "state": "queued" });
    let numeric = json!({ "id": 4214, "service": "torrent", "name": "Other", "state": "queued" });
    mount_listing(
        &server,
        &json!([transfer("4213", "downloading"), usenet, numeric]),
    )
    .await;

    let torrents = client(&server).list_torrents().await.expect("list");

    let ids: Vec<&str> = torrents.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["4213", "4214"]);
}

#[tokio::test]
async fn listing_maps_transfer_fields() {
    let server = MockServer::start().await;
    mount_listing(&server, &json!([transfer("4213", "completed")])).await;

    let torrents = client(&server).list_torrents().await.expect("list");

    let torrent = &torrents[0];
    assert_eq!(torrent.hash, "4213");
    assert_eq!(torrent.filename, "Some.Release");
    assert_eq!(torrent.original_filename.as_deref(), Some("Some.Release"));
    assert_eq!((torrent.bytes, torrent.original_bytes), (1000, 1000));
    assert_eq!(torrent.host.as_deref(), Some("TORRENT/Some.Release"));
    assert_eq!(torrent.progress, 62);
    assert_eq!(torrent.status.as_deref(), Some("completed"));
    assert_eq!(torrent.message.as_deref(), Some("boom"));
    assert!(torrent.added.is_some());
    assert!(torrent.ended.is_some(), "completed transfers have ended");
    assert_eq!(torrent.speed, Some(100), "600 bytes left in 6 seconds");
    assert_eq!(torrent.seeders, Some(9));
    let files = torrent.files.as_deref().expect("files");
    assert_eq!(files.len(), 2);
    assert_eq!((files[0].id, files[0].selected), (0, true));
    assert_eq!(
        (files[1].id, files[1].selected),
        (1, false),
        "partial files are not selected"
    );
    assert_eq!(files[1].path, "Some.Release/Some.Release.part");
}

#[tokio::test]
async fn listing_has_no_speed_or_end_while_the_eta_is_unknown() {
    let server = MockServer::start().await;
    let mut queued = transfer("4213", "queued");
    queued["eta"] = Value::Null;
    mount_listing(&server, &json!([queued])).await;

    let torrents = client(&server).list_torrents().await.expect("list");

    assert_eq!(torrents[0].speed, None);
    assert_eq!(torrents[0].ended, None);
}

#[tokio::test]
async fn missing_endpoint_answer_is_an_empty_listing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let torrents = client(&server).list_torrents().await.expect("list");

    assert_eq!(torrents.len(), 0);
}

#[tokio::test]
async fn blank_token_is_not_configured_and_sends_nothing() {
    let server = MockServer::start().await;

    let error = client_with_key(&server, "")
        .list_torrents()
        .await
        .expect_err("no token");

    assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
    assert_eq!(server.received_requests().await.expect("recorded").len(), 0);
}

async fn listing_error(response: ResponseTemplate) -> DebridError {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(response)
        .mount(&server)
        .await;
    client(&server).list_torrents().await.expect_err("failure")
}

#[tokio::test]
async fn invalid_tokens_report_an_authentication_failure() {
    for code in ["invalid_token", "authentication_required"] {
        let body = json!({ "error": code, "message": "Token rejected." });
        let error = listing_error(json_response(401, &body)).await;

        assert!(matches!(error, DebridError::NotConfigured(_)), "{error:?}");
        assert_eq!(
            error.to_string(),
            format!(
                "HIGHWAY authentication failed ({code}). Check the API token in the provider settings. Token rejected."
            )
        );
    }
}

#[tokio::test]
async fn foreign_tokens_report_the_wrong_account() {
    let body = json!({ "error": "system_feature_unavailable" });
    let error = listing_error(json_response(403, &body)).await;

    assert_eq!(
        error.to_string(),
        "The HIGHWAY token does not belong to a high-way.me account (system_feature_unavailable)."
    );
}

#[tokio::test]
async fn other_failures_report_status_code_and_message() {
    let body = json!({ "error": "internal", "message": "Database down" });
    let error = listing_error(json_response(500, &body)).await;

    assert!(matches!(error, DebridError::Provider(_)), "{error:?}");
    assert_eq!(
        error.to_string(),
        "HIGHWAY request failed (500 internal): Database down"
    );
}

#[tokio::test]
async fn failures_without_error_body_report_the_raw_body() {
    let error = listing_error(ResponseTemplate::new(502).set_body_string("Bad gateway")).await;

    assert_eq!(
        error.to_string(),
        "HIGHWAY request failed (502): Bad gateway"
    );
}

#[tokio::test]
async fn too_many_requests_is_a_rate_limit() {
    let error = listing_error(ResponseTemplate::new(429)).await;

    assert_eq!(error.retry_after(), Some(Duration::from_secs(120)));
}

async fn mount_stats(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(ENDPOINT))
        .and(query_param("stats", "1"))
        .respond_with(response)
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn user_uses_the_username_from_the_statistics() {
    let server = MockServer::start().await;
    let stats = json!({ "username": "maxmuster", "plan": "Pro", "premium": true, "plan_expires_at": 1_761_955_200 });
    mount_stats(&server, json_response(200, &stats)).await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("maxmuster"));
    assert_eq!(
        user.expiration,
        Some(Timestamp::from_second(1_761_955_200).expect("valid"))
    );
    assert!(user.premium);
}

#[tokio::test]
async fn user_has_no_expiration_when_the_plan_does_not_expire() {
    let server = MockServer::start().await;
    let stats = json!({ "username": "maxmuster", "plan_expires_at": null });
    mount_stats(&server, json_response(200, &stats)).await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.expiration, None);
    assert!(!user.premium);
}

#[tokio::test]
async fn user_falls_back_to_the_plan_without_username() {
    let server = MockServer::start().await;
    mount_stats(
        &server,
        json_response(200, &json!({ "username": " ", "plan": "Pro" })),
    )
    .await;

    let user = client(&server).user().await.expect("user");

    assert_eq!(user.username.as_deref(), Some("Pro"));
}

#[tokio::test]
async fn user_falls_back_to_a_stub_when_statistics_are_unavailable() {
    for response in [
        ResponseTemplate::new(200).set_body_string("<html>old server</html>"),
        ResponseTemplate::new(404),
        ResponseTemplate::new(200),
    ] {
        let server = MockServer::start().await;
        mount_stats(&server, response).await;

        let user = client(&server).user().await.expect("user");

        assert_eq!(user.username.as_deref(), Some("HIGHWAY"));
        assert_eq!(user.expiration, None);
    }
}

async fn mount_add(server: &MockServer, part: &str, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(ENDPOINT))
        .and(body_string_contains(part))
        .respond_with(response)
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn adding_a_magnet_returns_the_transfer_or_spool_id() {
    for id in ["4213", SPOOL_ID] {
        let server = MockServer::start().await;
        let created = json!({ "id": id, "service": "torrent", "state": "queued" });
        mount_add(&server, "name=\"magnet\"", json_response(200, &created)).await;

        let result = client(&server)
            .add_magnet("magnet:?xt=urn:btih:0123")
            .await
            .expect("added");

        assert_eq!(result, id);
    }
}

#[tokio::test]
async fn adding_fails_when_no_transfer_id_is_returned() {
    for response in [
        ResponseTemplate::new(200),
        json_response(200, &json!({ "state": "queued" })),
    ] {
        let server = MockServer::start().await;
        mount_add(&server, "magnet:?xt=urn:btih:0123", response).await;

        let error = client(&server)
            .add_magnet("magnet:?xt=urn:btih:0123")
            .await
            .expect_err("no id");

        assert!(
            matches!(error, DebridError::InvalidResponse(_)),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "HIGHWAY API did not return a transfer id."
        );
    }
}

#[tokio::test]
async fn adding_a_torrent_file_uploads_it_as_a_file_part() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(ENDPOINT))
        .and(body_string_contains(
            "name=\"file\"; filename=\"file.torrent\"",
        ))
        .and(body_string_contains("application/octet-stream"))
        .and(body_string_contains("d8:announce0:e"))
        .respond_with(json_response(200, &json!({ "id": 4213 })))
        .expect(1)
        .mount(&server)
        .await;

    let id = client(&server)
        .add_torrent_file(b"d8:announce0:e")
        .await
        .expect("added");

    assert_eq!(id, "4213");
}

#[tokio::test]
async fn adding_maps_rate_limit_messages_to_a_two_minute_rate_limit() {
    let server = MockServer::start().await;
    let refused = json!({ "error": "slow_down", "message": "rate limit exceeded" });
    mount_add(&server, "magnet", json_response(400, &refused)).await;

    let error = client(&server)
        .add_magnet("magnet:?")
        .await
        .expect_err("rate limited");

    assert_eq!(error.retry_after(), Some(Duration::from_secs(120)));
}

async fn deleted_query(rd_id: &str) -> Option<String> {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(ENDPOINT))
        .respond_with(json_response(200, &json!({ "removed": true })))
        .expect(1)
        .mount(&server)
        .await;
    let torrent = Torrent {
        rd_id: Some(rd_id.to_owned()),
        ..Torrent::default()
    };

    client(&server).delete(&torrent).await.expect("deleted");

    let requests = server.received_requests().await.expect("recorded");
    requests[0].url.query().map(str::to_owned)
}

#[tokio::test]
async fn deleting_a_spool_reference_omits_the_service() {
    assert_eq!(
        deleted_query(SPOOL_ID).await,
        Some(format!("id={SPOOL_ID}"))
    );
}

#[tokio::test]
async fn deleting_a_numeric_transfer_names_the_torrent_service() {
    assert_eq!(
        deleted_query("4213").await.as_deref(),
        Some("id=4213&service=torrent")
    );
}

#[tokio::test]
async fn deleting_a_torrent_without_provider_id_sends_nothing() {
    let server = MockServer::start().await;

    client(&server)
        .delete(&Torrent::default())
        .await
        .expect("nothing to delete");

    assert_eq!(server.received_requests().await.expect("recorded").len(), 0);
}

fn listed(state: &str) -> RemoteTorrent {
    RemoteTorrent {
        id: "4213".to_owned(),
        filename: "Some.Release".to_owned(),
        status: Some(state.to_owned()),
        message: Some("boom".to_owned()),
        bytes: 8123,
        progress: 50,
        host: Some("TORRENT/Some.Release".to_owned()),
        ..RemoteTorrent::default()
    }
}

fn with_id(id: &str) -> Torrent {
    Torrent {
        rd_id: Some(id.to_owned()),
        ..Torrent::default()
    }
}

async fn refreshed(
    server: &MockServer,
    mut torrent: Torrent,
    remote: Option<&RemoteTorrent>,
) -> Torrent {
    client(server)
        .refresh(&mut torrent, remote)
        .await
        .expect("refreshed");
    torrent
}

async fn mount_transfer(server: &MockServer, id: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(ENDPOINT))
        .and(query_param("id", id))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn refresh_maps_transfer_states() {
    let server = MockServer::start().await;
    for (state, expected) in [
        ("queued", TorrentStatus::Processing),
        ("downloading", TorrentStatus::Downloading),
        ("completed", TorrentStatus::Finished),
        ("error", TorrentStatus::Error),
    ] {
        let torrent = refreshed(&server, with_id("4213"), Some(&listed(state))).await;

        assert_eq!(torrent.rd_status, Some(expected), "state {state}");
        assert_eq!(torrent.rd_status_raw.as_deref(), Some(state));
        let error = (state == "error").then_some("boom");
        assert_eq!(torrent.error.as_deref(), error, "state {state}");
    }
    assert_eq!(server.received_requests().await.expect("recorded").len(), 0);
}

#[tokio::test]
async fn refresh_keeps_the_previous_status_for_unknown_states() {
    let server = MockServer::start().await;
    for (previous, expected) in [
        (None, TorrentStatus::Processing),
        (Some(TorrentStatus::Queued), TorrentStatus::Processing),
        (Some(TorrentStatus::Downloading), TorrentStatus::Downloading),
    ] {
        let torrent = Torrent {
            rd_status: previous,
            ..with_id("4213")
        };

        let torrent = refreshed(&server, torrent, Some(&listed("paused"))).await;

        assert_eq!(torrent.rd_status, Some(expected), "previous {previous:?}");
    }
}

#[tokio::test]
async fn refresh_stores_the_content_path_in_the_host() {
    let server = MockServer::start().await;

    let torrent = refreshed(&server, with_id("4213"), Some(&listed("downloading"))).await;

    assert_eq!(torrent.rd_host.as_deref(), Some("TORRENT/Some.Release"));
    assert_eq!(torrent.client_kind, Some(Provider::Highway));
    assert_eq!(torrent.rd_name.as_deref(), Some("Some.Release"));
    assert_eq!(torrent.rd_size, Some(8123));
    assert_eq!(torrent.rd_progress, Some(50));
}

#[tokio::test]
async fn refresh_keeps_the_first_added_and_ended_times() {
    let server = MockServer::start().await;
    let earlier = Timestamp::from_second(1_700_000_000).expect("valid");
    let torrent = Torrent {
        rd_added: Some(earlier),
        ..with_id("4213")
    };
    let completed = RemoteTorrent {
        ended: Some(Timestamp::now()),
        ..listed("completed")
    };

    let torrent = refreshed(&server, torrent, Some(&completed)).await;
    let ended = torrent.rd_ended.expect("completed transfers end");
    let again = refreshed(
        &server,
        torrent,
        Some(&RemoteTorrent {
            ended: Some(Timestamp::now()),
            ..listed("completed")
        }),
    )
    .await;

    assert_eq!(again.rd_added, Some(earlier));
    assert_eq!(again.rd_ended, Some(ended));
}

#[tokio::test]
async fn refresh_looks_the_transfer_up_without_a_listing_entry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ENDPOINT))
        .and(query_param("id", "4213"))
        .and(query_param("service", "torrent"))
        .respond_with(json_response(200, &transfer("4213", "downloading")))
        .expect(1)
        .mount(&server)
        .await;

    let torrent = refreshed(&server, with_id("4213"), None).await;

    assert_eq!(torrent.rd_status, Some(TorrentStatus::Downloading));
    assert_eq!(torrent.rd_speed, Some(100));
    assert_eq!(torrent.rd_seeders, Some(9));
    assert!(torrent.rd_added.is_some());
    assert_eq!(torrent.files().len(), 2);
}

#[tokio::test]
async fn refresh_looks_spool_references_up_without_a_service() {
    let server = MockServer::start().await;
    mount_transfer(
        &server,
        SPOOL_ID,
        json_response(200, &transfer(SPOOL_ID, "queued")),
    )
    .await;

    let torrent = refreshed(&server, with_id(SPOOL_ID), None).await;

    assert_eq!(torrent.rd_status, Some(TorrentStatus::Processing));
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(
        requests[0].url.query(),
        Some(format!("id={SPOOL_ID}").as_str())
    );
}

#[tokio::test]
async fn refresh_marks_the_torrent_deleted_when_the_transfer_is_gone() {
    let server = MockServer::start().await;
    mount_transfer(&server, "4213", ResponseTemplate::new(404)).await;
    let torrent = Torrent {
        rd_status: Some(TorrentStatus::Downloading),
        ..with_id("4213")
    };

    let torrent = refreshed(&server, torrent, None).await;

    assert_eq!(torrent.rd_status_raw.as_deref(), Some("deleted"));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Downloading));
}

#[tokio::test]
async fn unrestrict_resolves_the_zip_reference_to_a_fresh_signed_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(ENDPOINT))
        .and(query_param("id", "4213"))
        .and(query_param("service", "torrent"))
        .respond_with(json_response(200, &transfer("4213", "completed")))
        .expect(1)
        .mount(&server)
        .await;

    let link = client(&server)
        .unrestrict("highway://4213/torrent/zip")
        .await
        .expect("resolved");

    assert_eq!(
        link,
        "https://nl.dwld.link/dav/TORRENT/Some.Release/?download=zip"
    );
}

#[tokio::test]
async fn unrestrict_leaves_per_file_references_and_real_urls_unchanged() {
    let server = MockServer::start().await;
    let real = "https://nl.dwld.link/dav/TORRENT/Some.Release/a.mkv?zip_user_id=1&zip_expires=1&zip_signature=abc";

    for link in ["highway://4213/torrent/2", real] {
        let resolved = client(&server).unrestrict(link).await.expect("unchanged");

        assert_eq!(resolved, link);
    }
    assert_eq!(server.received_requests().await.expect("recorded").len(), 0);
}

#[tokio::test]
async fn unrestrict_fails_without_a_zip_download_url() {
    let server = MockServer::start().await;
    mount_transfer(
        &server,
        "4213",
        json_response(200, &json!({ "id": "4213", "download_url": null })),
    )
    .await;

    let error = client(&server)
        .unrestrict("highway://4213/torrent/zip")
        .await
        .expect_err("no url");

    assert!(
        error.to_string().contains("did not return a download URL"),
        "{error}"
    );
}

#[tokio::test]
async fn unrestrict_reports_a_transfer_that_is_gone() {
    let server = MockServer::start().await;
    mount_transfer(&server, "4213", ResponseTemplate::new(404)).await;

    let error = client(&server)
        .unrestrict("highway://4213/torrent/zip")
        .await
        .expect_err("gone");

    assert!(error.is_not_found(), "{error:?}");
    assert!(error.to_string().contains("was not found"), "{error}");
    assert!(
        error.to_string().contains("remove it here and re-add it"),
        "{error}"
    );
}

#[tokio::test]
async fn unrestrict_rejects_empty_and_malformed_links() {
    let server = MockServer::start().await;

    let empty = client(&server).unrestrict(" ").await.expect_err("empty");
    let malformed = client(&server)
        .unrestrict("highway://4213/zip")
        .await
        .expect_err("malformed");

    assert!(matches!(empty, DebridError::Provider(_)), "{empty:?}");
    assert_eq!(
        malformed.to_string(),
        "Invalid HIGHWAY link format: highway://4213/zip"
    );
}

const HASH: &str = "3641ad15ad451518f558d5ce5b76be4a618911c1";

async fn mount_check(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(ENDPOINT))
        .and(query_param("check", HASH))
        .and(query_param("service", "torrent"))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn available_files_list_a_ready_cached_transfer_linked_to_the_account() {
    let server = MockServer::start().await;
    let check = json!({ "service": "torrent", "results": { HASH: { "known": true, "ready": true, "id": "4213" } } });
    mount_check(&server, json_response(200, &check)).await;
    let mut linked = transfer("4213", "completed");
    linked["files"]
        .as_array_mut()
        .expect("files")
        .push(json!({ "path": "", "size": 1 }));
    mount_transfer(&server, "4213", json_response(200, &linked)).await;

    let files = client(&server).available_files(HASH).await.expect("files");

    let listed: Vec<(&str, i64)> = files
        .iter()
        .map(|f| (f.filename.as_str(), f.filesize))
        .collect();
    assert_eq!(
        listed,
        [
            ("Some.Release/Some.Release.mkv", 990),
            ("Some.Release/Some.Release.part", 10)
        ]
    );
}

#[tokio::test]
async fn available_files_are_empty_while_the_cache_is_not_ready() {
    let server = MockServer::start().await;
    let check = json!({ "results": { HASH: { "known": true, "ready": false } } });
    mount_check(&server, json_response(200, &check)).await;

    let files = client(&server).available_files(HASH).await.expect("files");

    assert_eq!(files.len(), 0);
}

#[tokio::test]
async fn available_files_are_empty_when_the_server_has_no_cache_lookup() {
    let server = MockServer::start().await;
    mount_check(
        &server,
        ResponseTemplate::new(200).set_body_string("<html></html>"),
    )
    .await;

    let files = client(&server).available_files(HASH).await.expect("files");

    assert_eq!(files.len(), 0);
}

#[tokio::test]
async fn available_files_ignore_hashes_that_are_not_hex() {
    let server = MockServer::start().await;

    let files = client(&server)
        .available_files("not-a-hash")
        .await
        .expect("files");

    assert_eq!(files.len(), 0);
    assert_eq!(server.received_requests().await.expect("recorded").len(), 0);
}

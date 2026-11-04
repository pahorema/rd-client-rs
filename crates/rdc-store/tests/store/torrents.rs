use jiff::Timestamp;
use rdc_model::{
    DownloadClient, Provider, Torrent, TorrentDownloadAction, TorrentFinishedAction,
    TorrentHostDownloadAction, TorrentStatus,
};
use rdc_store::{NewTorrent, RetryOnError};
use uuid::Uuid;

use crate::common::TestStore;

fn template() -> Torrent {
    Torrent {
        category: Some("tv".to_owned()),
        host_download_action: TorrentHostDownloadAction::DownloadNone,
        finished_action_delay: 4,
        download_action: TorrentDownloadAction::DownloadManual,
        finished_action: TorrentFinishedAction::RemoveClient,
        download_min_size: 5,
        include_regex: Some("inc".to_owned()),
        exclude_regex: Some("exc".to_owned()),
        download_manual_files: Some("a.mkv,b.mkv".to_owned()),
        priority: Some(2),
        torrent_retry_attempts: 3,
        download_retry_attempts: 6,
        delete_on_error: 7,
        lifetime: 8,
        rd_status: Some(TorrentStatus::Queued),
        rd_name: Some("Name".to_owned()),
        // Not copied by `add`.
        download_client: DownloadClient::Symlink,
        error: Some("ignored".to_owned()),
        rd_size: Some(99),
        retry_count: 9,
        ..Torrent::default()
    }
}

async fn insert_download(test: &TestStore, torrent_id: Uuid, path: &str, error: Option<&str>) {
    sqlx::query(
        r#"INSERT INTO "Downloads" ("DownloadId", "TorrentId", "Path", "Added", "RetryCount", "Error")
           VALUES (?, ?, ?, '2026-01-01 00:00:00+00:00', 0, ?)"#,
    )
    .bind(Uuid::new_v4().hyphenated().to_string().to_uppercase())
    .bind(torrent_id.hyphenated().to_string().to_uppercase())
    .bind(path)
    .bind(error)
    .execute(&test.raw().await)
    .await
    .expect("insert download");
}

#[tokio::test]
async fn add_creates_the_payload_row_and_get_returns_it() {
    let test = TestStore::new().await;
    let template = Torrent::default();
    let added = test
        .store
        .torrents()
        .add(&NewTorrent {
            rd_id: None,
            hash: "hash-1",
            payload: Some("magnet:?xt=urn:btih:hash-1"),
            is_file: false,
            download_client: DownloadClient::Internal,
            template: &template,
        })
        .await
        .expect("add");

    let content: String = sqlx::query_scalar(r#"SELECT "Content" FROM "TorrentPayloads""#)
        .fetch_one(&test.raw().await)
        .await
        .expect("payload row");
    assert_eq!(content, "magnet:?xt=urn:btih:hash-1");
    let stored = test
        .store
        .torrents()
        .get(added.torrent_id)
        .await
        .expect("get")
        .expect("found");
    assert_eq!(
        stored.payload.as_deref(),
        Some("magnet:?xt=urn:btih:hash-1")
    );
    assert_eq!(stored, added);
}

#[tokio::test]
async fn add_copies_the_template_and_lowercases_the_hash() {
    let test = TestStore::new().await;
    let template = template();
    let before = Timestamp::now();
    let added = test
        .store
        .torrents()
        .add(&NewTorrent {
            rd_id: Some("RD1"),
            hash: "ABCDEF",
            payload: Some("   "),
            is_file: true,
            download_client: DownloadClient::Aria2c,
            template: &template,
        })
        .await
        .expect("add");

    let expected = Torrent {
        torrent_id: added.torrent_id,
        added: added.added,
        hash: "abcdef".to_owned(),
        rd_id: Some("RD1".to_owned()),
        is_file: true,
        download_client: DownloadClient::Aria2c,
        error: None,
        rd_size: None,
        retry_count: 0,
        ..template
    };
    assert_eq!(added, expected);
    assert!(
        added.added
            >= before
                .checked_sub(jiff::SignedDuration::from_micros(1))
                .expect("range")
    );
    let stored = test
        .store
        .torrents()
        .get(added.torrent_id)
        .await
        .expect("get")
        .expect("found");
    assert_eq!(stored, expected, "blank payloads are not stored");

    let raw: (String, String, i64, i64) =
        sqlx::query_as(r#"SELECT "TorrentId", "Added", "Type", "IsFile" FROM "Torrents""#)
            .fetch_one(&test.raw().await)
            .await
            .expect("raw row");
    assert_eq!(
        raw.0,
        added.torrent_id.hyphenated().to_string().to_uppercase()
    );
    assert!(
        raw.1.ends_with("+00:00") && raw.1.as_bytes()[10] == b' ',
        "stored as {}",
        raw.1
    );
    assert_eq!((raw.2, raw.3), (0, 1));
}

#[tokio::test]
async fn list_never_carries_payloads() {
    let test = TestStore::new().await;
    let template = Torrent::default();
    let payload = "A".repeat(1024);
    test.store
        .torrents()
        .add(&NewTorrent {
            rd_id: None,
            hash: "hash-2",
            payload: Some(&payload),
            is_file: true,
            download_client: DownloadClient::Internal,
            template: &template,
        })
        .await
        .expect("add");

    let torrents = test.store.torrents().list().await.expect("list");
    assert_eq!(torrents.len(), 1);
    assert_eq!(torrents[0].payload, None);
}

#[tokio::test]
async fn delete_removes_the_torrent_its_downloads_and_payload() {
    let test = TestStore::new().await;
    let template = Torrent::default();
    let torrents = test.store.torrents();
    let added = torrents
        .add(&NewTorrent {
            rd_id: None,
            hash: "hash-3",
            payload: Some("magnet:?xt=urn:btih:hash-3"),
            is_file: false,
            download_client: DownloadClient::Internal,
            template: &template,
        })
        .await
        .expect("add");
    insert_download(&test, added.torrent_id, "https://host/1", None).await;

    torrents.delete(added.torrent_id).await.expect("delete");
    torrents
        .delete(added.torrent_id)
        .await
        .expect("deleting again is fine");
    torrents
        .delete(Uuid::new_v4())
        .await
        .expect("deleting a missing torrent is fine");

    let raw = test.raw().await;
    for table in ["Torrents", "Downloads", "TorrentPayloads"] {
        let sql = format!(r#"SELECT COUNT(*) FROM "{table}""#);
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .fetch_one(&raw)
            .await
            .expect("count");
        assert_eq!(count, 0, "{table} is empty");
    }
}

#[tokio::test]
async fn field_updates_touch_only_their_columns() {
    let test = TestStore::new().await;
    let torrents = test.store.torrents();
    let template = template();
    let added = torrents
        .add(&NewTorrent {
            rd_id: None,
            hash: "hash",
            payload: None,
            is_file: false,
            download_client: DownloadClient::Internal,
            template: &template,
        })
        .await
        .expect("add");
    let id = added.torrent_id;
    let at: Timestamp = "2026-05-28T16:30:00.5Z".parse().expect("instant");

    torrents.update_rd_id(id, "RD2").await.expect("rd id");
    torrents.update_hash(id, "NEWHASH").await.expect("hash");
    torrents.update_category(id, None).await.expect("category");
    torrents
        .update_files_selected(id, at)
        .await
        .expect("files selected");
    torrents.update_priority(id, None).await.expect("priority");
    torrents.update_error(id, "broken").await.expect("error");
    torrents.update_retry(id, Some(at), 2).await.expect("retry");

    let mut provider = added.clone();
    provider.client_kind = Some(Provider::AllDebrid);
    provider.rd_name = Some("Provider name".to_owned());
    provider.rd_size = Some(10);
    provider.rd_host = Some("host".to_owned());
    provider.rd_split = Some(1);
    provider.rd_progress = Some(50);
    provider.rd_status = Some(TorrentStatus::Downloading);
    provider.rd_status_raw = Some("downloading".to_owned());
    provider.rd_added = Some(at);
    provider.rd_ended = Some(at);
    provider.rd_speed = Some(100);
    provider.rd_seeders = Some(3);
    provider.rd_files = Some("[]".to_owned());
    provider.category = Some("must not be written".to_owned());
    provider.rd_id = Some("must not be written".to_owned());
    torrents.update_rd_data(&provider).await.expect("rd data");

    let mut options = added.clone();
    options.download_client = DownloadClient::DownloadStation;
    options.host_download_action = TorrentHostDownloadAction::DownloadAll;
    options.category = Some("movies".to_owned());
    options.priority = Some(1);
    options.download_retry_attempts = 11;
    options.torrent_retry_attempts = 12;
    options.delete_on_error = 13;
    options.lifetime = 14;
    options.rd_name = Some("must not be written".to_owned());
    options.error = Some("must not be written".to_owned());
    torrents.update(&options).await.expect("options");

    let stored = torrents.get(id).await.expect("get").expect("found");
    let expected = Torrent {
        rd_id: Some("RD2".to_owned()),
        hash: "newhash".to_owned(),
        files_selected: Some(at),
        error: Some("broken".to_owned()),
        retry: Some(at),
        retry_count: 2,
        download_client: DownloadClient::DownloadStation,
        host_download_action: TorrentHostDownloadAction::DownloadAll,
        category: Some("movies".to_owned()),
        priority: Some(1),
        download_retry_attempts: 11,
        torrent_retry_attempts: 12,
        delete_on_error: 13,
        lifetime: 14,
        rd_name: Some("Provider name".to_owned()),
        ..provider
    };
    assert_eq!(stored, expected);

    let missing = Uuid::new_v4();
    torrents
        .update_error(missing, "x")
        .await
        .expect("missing torrents are ignored");
    torrents
        .update_rd_data(&Torrent::default())
        .await
        .expect("missing torrents are ignored");
}

#[tokio::test]
async fn completing_counts_failed_downloads_and_schedules_a_retry() {
    let test = TestStore::new().await;
    let torrents = test.store.torrents();
    let template = Torrent {
        torrent_retry_attempts: 1,
        ..Torrent::default()
    };
    let added = torrents
        .add(&NewTorrent {
            rd_id: None,
            hash: "hash",
            payload: None,
            is_file: false,
            download_client: DownloadClient::Internal,
            template: &template,
        })
        .await
        .expect("add");
    let id = added.torrent_id;
    insert_download(&test, id, "https://host/1", Some("timeout")).await;
    insert_download(&test, id, "https://host/2", Some("  ")).await;
    insert_download(&test, id, "https://host/3", None).await;
    let at: Timestamp = "2026-05-28T16:30:00Z".parse().expect("instant");

    let before = Timestamp::now();
    torrents
        .update_complete(id, Some(" "), Some(at), RetryOnError::Schedule)
        .await
        .expect("complete");
    let stored = torrents.get(id).await.expect("get").expect("found");
    assert_eq!(
        stored.error.as_deref(),
        Some("1/3 downloads failed with errors")
    );
    assert_eq!(stored.completed, Some(at));
    assert_eq!(stored.retry_count, 1);
    assert!(stored.retry.is_some_and(|retry| {
        retry
            >= before
                .checked_sub(jiff::SignedDuration::from_micros(1))
                .expect("range")
    }));

    // Attempts used up: the error is stored, the retry state kept.
    let first_retry = stored.retry;
    torrents
        .update_complete(id, Some("failed"), None, RetryOnError::Schedule)
        .await
        .expect("complete");
    let stored = torrents.get(id).await.expect("get").expect("found");
    assert_eq!(
        (stored.error.as_deref(), stored.completed),
        (Some("failed"), None)
    );
    assert_eq!((stored.retry_count, stored.retry), (1, first_retry));
}

#[tokio::test]
async fn completing_without_errors_clears_or_keeps_the_given_text() {
    let test = TestStore::new().await;
    let torrents = test.store.torrents();
    let template = Torrent {
        torrent_retry_attempts: 5,
        ..Torrent::default()
    };
    let added = torrents
        .add(&NewTorrent {
            rd_id: None,
            hash: "hash",
            payload: None,
            is_file: false,
            download_client: DownloadClient::Internal,
            template: &template,
        })
        .await
        .expect("add");
    let id = added.torrent_id;
    let at: Timestamp = "2026-05-28T16:30:00Z".parse().expect("instant");

    torrents
        .update_complete(id, Some("  "), Some(at), RetryOnError::Schedule)
        .await
        .expect("blank");
    let stored = torrents.get(id).await.expect("get").expect("found");
    assert_eq!(
        (stored.error.as_deref(), stored.retry_count, stored.retry),
        (Some("  "), 0, None)
    );

    torrents
        .update_complete(id, Some("boom"), Some(at), RetryOnError::Skip)
        .await
        .expect("skip");
    let stored = torrents.get(id).await.expect("get").expect("found");
    assert_eq!(
        (stored.error.as_deref(), stored.retry_count, stored.retry),
        (Some("boom"), 0, None)
    );

    torrents
        .update_complete(id, None, None, RetryOnError::Schedule)
        .await
        .expect("clear");
    let stored = torrents.get(id).await.expect("get").expect("found");
    assert_eq!(
        (stored.error, stored.completed, stored.retry_count),
        (None, None, 0)
    );
    torrents
        .update_complete(Uuid::new_v4(), Some("x"), None, RetryOnError::Schedule)
        .await
        .expect("missing torrents are ignored");
}

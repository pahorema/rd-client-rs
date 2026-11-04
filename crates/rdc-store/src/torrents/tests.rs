use jiff::Timestamp;
use rdc_model::{
    DownloadClient, Provider, TorrentDownloadAction, TorrentFinishedAction,
    TorrentHostDownloadAction, TorrentStatus,
};

use super::*;
use crate::{
    StoreError,
    test_support::{execute, insert_download_row, insert_torrent_row, temp_store},
};

const FIRST: &str = "0A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10";
const SECOND: &str = "1A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10";
const THIRD: &str = "2A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10";
const FOURTH: &str = "3A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10";

fn id(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("valid uuid")
}

fn instant(text: &str) -> Timestamp {
    text.parse().expect("valid instant")
}

#[tokio::test]
async fn list_orders_by_priority_then_added_instant() {
    let (_dir, store) = temp_store().await;
    let pool = &store.pool;
    insert_torrent_row(pool, FIRST, "a", "2026-01-01 10:00:00+00:00", 0).await;
    insert_torrent_row(pool, SECOND, "b", "2026-01-01 11:00:00+02:00", 0).await;
    insert_torrent_row(pool, THIRD, "c", "2026-01-01 09:30:00+00:00", 0).await;
    insert_torrent_row(pool, FOURTH, "d", "2026-01-01 08:00:00+00:00", 0).await;
    execute(
        pool,
        r#"UPDATE "Torrents" SET "Priority" = 1
           WHERE "TorrentId" IN ('1A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10',
                                 '2A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10')"#,
    )
    .await;

    let hashes: Vec<String> = store
        .torrents()
        .list()
        .await
        .expect("list")
        .into_iter()
        .map(|torrent| torrent.hash)
        .collect();
    // 11:00+02:00 is 09:00 UTC, before 09:30 UTC; torrents without a
    // priority come last.
    assert_eq!(hashes, ["b", "c", "d", "a"]);
}

#[tokio::test]
async fn list_groups_downloads_and_ignores_other_row_types() {
    let (_dir, store) = temp_store().await;
    let pool = &store.pool;
    insert_torrent_row(pool, FIRST, "a", "2026-01-01 10:00:00+00:00", 0).await;
    insert_torrent_row(pool, SECOND, "b", "2026-01-01 11:00:00+00:00", 1).await;
    insert_download_row(pool, THIRD, FIRST, "https://host/1").await;
    insert_download_row(pool, FOURTH, SECOND, "https://host/2").await;
    execute(
        pool,
        r#"INSERT INTO "TorrentPayloads" VALUES ('0A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10', 'magnet')"#,
    )
    .await;

    let torrents = store.torrents().list().await.expect("list");
    assert_eq!(torrents.len(), 1);
    assert_eq!(torrents[0].torrent_id, id(FIRST));
    assert_eq!(torrents[0].payload, None);
    let paths: Vec<&str> = torrents[0]
        .downloads
        .iter()
        .map(|d| d.path.as_str())
        .collect();
    assert_eq!(paths, ["https://host/1"]);
    assert_eq!(store.torrents().get(id(SECOND)).await.expect("get"), None);
    assert_eq!(
        store.torrents().payload(id(SECOND)).await.expect("payload"),
        None
    );
}

#[tokio::test]
async fn get_decodes_every_column_with_downloads_and_payload() {
    let (_dir, store) = temp_store().await;
    let pool = &store.pool;
    insert_torrent_row(pool, FIRST, "abc", "2026-05-28 16:30:00.1234567+00:00", 0).await;
    execute(
        pool,
        r#"UPDATE "Torrents" SET "Category" = 'tv', "DownloadAction" = 2, "FinishedAction" = 3,
               "FinishedActionDelay" = 4, "HostDownloadAction" = 1, "DownloadMinSize" = 5,
               "IncludeRegex" = 'inc', "ExcludeRegex" = 'exc', "DownloadManualFiles" = 'a,b',
               "DownloadClient" = 3, "FilesSelected" = '2026-05-28T17:00:00',
               "Completed" = '2026-05-28 19:00:00+02:00', "Retry" = '2026-05-28 17:30:00+00:00',
               "IsFile" = 1, "Priority" = 7, "RetryCount" = 1, "DownloadRetryAttempts" = 3,
               "TorrentRetryAttempts" = 2, "DeleteOnError" = 6, "Lifetime" = 8, "Error" = 'oops',
               "ClientKind" = 3, "RdId" = 'rd1', "RdName" = 'Name', "RdSize" = 9000000000,
               "RdHost" = 'host', "RdSplit" = 10, "RdProgress" = 100, "RdStatus" = 99,
               "RdStatusRaw" = 'error', "RdAdded" = '2026-05-28 16:31:00+00:00',
               "RdEnded" = '2026-05-28 16:32:00+00:00', "RdSpeed" = 11, "RdSeeders" = 12,
               "RdFiles" = '[]';
           INSERT INTO "TorrentPayloads" VALUES ('0A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10', 'BASE64');"#,
    )
    .await;
    insert_download_row(pool, SECOND, FIRST, "https://host/1").await;

    let torrent = store
        .torrents()
        .get(id(FIRST))
        .await
        .expect("get")
        .expect("found");
    let expected = Torrent {
        torrent_id: id(FIRST),
        hash: "abc".to_owned(),
        category: Some("tv".to_owned()),
        download_action: TorrentDownloadAction::DownloadManual,
        finished_action: TorrentFinishedAction::RemoveClient,
        finished_action_delay: 4,
        host_download_action: TorrentHostDownloadAction::DownloadNone,
        download_min_size: 5,
        include_regex: Some("inc".to_owned()),
        exclude_regex: Some("exc".to_owned()),
        download_manual_files: Some("a,b".to_owned()),
        download_client: DownloadClient::DownloadStation,
        added: instant("2026-05-28T16:30:00.1234567Z"),
        files_selected: Some(instant("2026-05-28T17:00:00Z")),
        completed: Some(instant("2026-05-28T17:00:00Z")),
        retry: Some(instant("2026-05-28T17:30:00Z")),
        is_file: true,
        priority: Some(7),
        retry_count: 1,
        download_retry_attempts: 3,
        torrent_retry_attempts: 2,
        delete_on_error: 6,
        lifetime: 8,
        error: Some("oops".to_owned()),
        client_kind: Some(Provider::TorBox),
        rd_id: Some("rd1".to_owned()),
        rd_name: Some("Name".to_owned()),
        rd_size: Some(9_000_000_000),
        rd_host: Some("host".to_owned()),
        rd_split: Some(10),
        rd_progress: Some(100),
        rd_status: Some(TorrentStatus::Error),
        rd_status_raw: Some("error".to_owned()),
        rd_added: Some(instant("2026-05-28T16:31:00Z")),
        rd_ended: Some(instant("2026-05-28T16:32:00Z")),
        rd_speed: Some(11),
        rd_seeders: Some(12),
        rd_files: Some("[]".to_owned()),
        downloads: Vec::new(),
        payload: Some("BASE64".to_owned()),
    };
    assert_eq!(
        Torrent {
            downloads: Vec::new(),
            ..torrent.clone()
        },
        expected
    );
    assert_eq!(torrent.downloads.len(), 1);
    assert_eq!(torrent.downloads[0].download_id, id(SECOND));
    assert_eq!(torrent.downloads[0].torrent_id, id(FIRST));
    assert_eq!(
        store.torrents().get(id(THIRD)).await.expect("get missing"),
        None
    );
}

#[tokio::test]
async fn find_by_hash_lowercases_the_input_and_skips_the_payload() {
    let (_dir, store) = temp_store().await;
    let pool = &store.pool;
    insert_torrent_row(pool, FIRST, "abcdef", "2026-01-01 10:00:00+00:00", 0).await;
    insert_torrent_row(pool, SECOND, "UPPER", "2026-01-01 10:00:00+00:00", 0).await;
    insert_download_row(pool, THIRD, FIRST, "https://host/1").await;
    execute(
        pool,
        r#"INSERT INTO "TorrentPayloads" VALUES ('0A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10', 'magnet')"#,
    )
    .await;

    let torrents = store.torrents();
    let found = torrents
        .find_by_hash("ABCdef")
        .await
        .expect("find")
        .expect("found");
    assert_eq!(found.torrent_id, id(FIRST));
    assert_eq!(found.downloads.len(), 1);
    assert_eq!(found.payload, None);
    assert_eq!(torrents.find_by_hash("UPPER").await.expect("find"), None);
    assert_eq!(torrents.find_by_hash("missing").await.expect("find"), None);
    assert_eq!(
        torrents
            .payload(id(FIRST))
            .await
            .expect("payload")
            .as_deref(),
        Some("magnet")
    );
}

#[tokio::test]
async fn undecodable_values_are_reported() {
    let (_dir, store) = temp_store().await;
    insert_torrent_row(&store.pool, FIRST, "a", "2026-01-01 10:00:00+00:00", 0).await;
    execute(&store.pool, r#"UPDATE "Torrents" SET "RdStatus" = 42"#).await;
    let error = store.torrents().list().await.expect_err("invalid status");
    assert!(
        matches!(&error, StoreError::InvalidValue { column: "RdStatus", value } if value == "42"),
        "{error:?}"
    );
    execute(
        &store.pool,
        r#"UPDATE "Torrents" SET "RdStatus" = NULL, "Added" = 'yesterday'"#,
    )
    .await;
    let error = store
        .torrents()
        .list()
        .await
        .expect_err("invalid timestamp");
    assert!(
        matches!(
            error,
            StoreError::InvalidValue {
                column: "Added",
                ..
            }
        ),
        "{error:?}"
    );
}

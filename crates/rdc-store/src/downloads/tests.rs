use super::*;
use crate::test_support::{insert_download_row, insert_torrent_row, temp_store};

const TORRENT: &str = "0A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10";

fn info(link: &str) -> DownloadInfo {
    DownloadInfo {
        file_name: Some("file.mkv".to_owned()),
        restricted_link: link.to_owned(),
    }
}

async fn count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(r#"SELECT COUNT(*) FROM "Downloads""#)
        .fetch_one(pool)
        .await
        .expect("count")
}

#[tokio::test]
async fn a_duplicate_add_reports_already_exists() {
    let (_dir, store) = temp_store().await;
    insert_torrent_row(&store.pool, TORRENT, "a", "2026-01-01 00:00:00+00:00", 0).await;
    let torrent_id = Uuid::parse_str(TORRENT).expect("uuid");
    let downloads = store.downloads();

    let first = downloads.try_add(torrent_id, &info("https://host/a")).await;
    let second = downloads.try_add(torrent_id, &info("https://host/a")).await;
    assert_eq!(first.expect("first add"), AddResult::Added);
    assert_eq!(second.expect("second add"), AddResult::AlreadyExists);
    assert_eq!(count(&store.pool).await, 1);
}

#[tokio::test]
async fn an_add_for_a_missing_torrent_reports_torrent_missing() {
    let (_dir, store) = temp_store().await;
    let result = store
        .downloads()
        .try_add(Uuid::new_v4(), &info("https://host/a"))
        .await;
    assert_eq!(result.expect("add"), AddResult::TorrentMissing);
    assert_eq!(count(&store.pool).await, 0);
}

#[tokio::test]
async fn downloads_of_other_row_types_are_hidden() {
    let (_dir, store) = temp_store().await;
    let download = "1A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10";
    insert_torrent_row(&store.pool, TORRENT, "a", "2026-01-01 00:00:00+00:00", 1).await;
    insert_download_row(&store.pool, download, TORRENT, "https://host/a").await;
    let torrent_id = Uuid::parse_str(TORRENT).expect("uuid");
    let downloads = store.downloads();

    let found = downloads
        .get(Uuid::parse_str(download).expect("uuid"))
        .await;
    assert_eq!(found.expect("get"), None);
    let added = downloads.try_add(torrent_id, &info("https://host/b")).await;
    assert_eq!(added.expect("try add"), AddResult::TorrentMissing);
}

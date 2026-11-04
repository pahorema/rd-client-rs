//! Helpers shared by the unit tests.

use sqlx::SqlitePool;
use tempfile::TempDir;

use crate::Store;

/// A store on a new database in a temporary directory, kept alive by the
/// returned guard.
pub(crate) async fn temp_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open(dir.path().join("rd-client.db"))
        .await
        .expect("open store");
    (dir, store)
}

/// Inserts a torrent row with every required column set to 0 and the given
/// identity, hash, addition time and row type.
pub(crate) async fn insert_torrent_row(
    pool: &SqlitePool,
    torrent_id: &str,
    hash: &str,
    added: &str,
    row_type: i64,
) {
    sqlx::query(
        r#"INSERT INTO "Torrents" ("TorrentId", "Hash", "Added", "DeleteOnError",
               "DownloadAction", "DownloadClient", "DownloadMinSize", "DownloadRetryAttempts",
               "FinishedAction", "FinishedActionDelay", "HostDownloadAction", "IsFile", "Lifetime",
               "RetryCount", "TorrentRetryAttempts", "Type")
           VALUES (?, ?, ?, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, ?)"#,
    )
    .bind(torrent_id)
    .bind(hash)
    .bind(added)
    .bind(row_type)
    .execute(pool)
    .await
    .expect("insert torrent row");
}

/// Inserts a download row with the required columns.
pub(crate) async fn insert_download_row(
    pool: &SqlitePool,
    download_id: &str,
    torrent_id: &str,
    path: &str,
) {
    sqlx::query(
        r#"INSERT INTO "Downloads" ("DownloadId", "TorrentId", "Path", "Added", "RetryCount")
           VALUES (?, ?, ?, '2026-01-01 00:00:00+00:00', 0)"#,
    )
    .bind(download_id)
    .bind(torrent_id)
    .bind(path)
    .execute(pool)
    .await
    .expect("insert download row");
}

/// Runs one statement.
pub(crate) async fn execute(pool: &SqlitePool, sql: &'static str) {
    sqlx::raw_sql(sql).execute(pool).await.expect("sql runs");
}

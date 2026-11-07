//! Download rows: one file of a torrent transferred to the host.
//!
//! Field updates touch only their column and silently do nothing when the
//! download does not exist.

#[cfg(test)]
mod tests;

use jiff::Timestamp;
use rdc_model::{Download, DownloadInfo, Torrent};
use sqlx::{SqlitePool, error::DatabaseError, sqlite::SqliteRow};
use tracing::debug;
use uuid::Uuid;

use crate::{
    codec::{self, optional_timestamp_text, timestamp_text, uuid_text},
    error::{Result, StoreError},
    row, torrents,
};

/// Every column of `Downloads`, in [`from_row`] order.
macro_rules! download_columns {
    () => {
        r#""DownloadId", "TorrentId", "Path", "Link", "Added", "DownloadQueued", "DownloadStarted",
        "DownloadFinished", "UnpackingQueued", "UnpackingStarted", "UnpackingFinished",
        "Completed", "RetryCount", "Error", "RemoteId", "FileName""#
    };
}
pub(crate) use download_columns;

/// SQLite extended result code of a violated unique constraint.
const UNIQUE_VIOLATION: &str = "2067";

/// Outcome of [`Downloads::try_add`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddResult {
    /// The download was created.
    Added,
    /// The torrent already has a download for this restricted link.
    AlreadyExists,
    /// The torrent does not exist (any more).
    TorrentMissing,
    /// The restricted link is blank.
    InvalidInput,
}

/// A download together with its torrent (without the torrent's downloads
/// and payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadWithTorrent {
    /// The download.
    pub download: Download,
    /// The torrent it belongs to.
    pub torrent: Torrent,
}

/// Download operations; obtained from [`Store::downloads`](crate::Store::downloads).
#[derive(Debug, Clone, Copy)]
pub struct Downloads<'store> {
    pool: &'store SqlitePool,
}

impl<'store> Downloads<'store> {
    pub(crate) fn new(pool: &'store SqlitePool) -> Self {
        Self { pool }
    }

    /// The downloads of a torrent, oldest first.
    ///
    /// # Errors
    ///
    /// Fails on database errors and undecodable rows.
    pub async fn for_torrent(&self, torrent_id: Uuid) -> Result<Vec<Download>> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            download_columns!(),
            r#" FROM "Downloads" WHERE "TorrentId" = ? ORDER BY rowid"#
        ))
        .bind(uuid_text(torrent_id))
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(from_row).collect()
    }

    /// The download with this id and its torrent.
    ///
    /// # Errors
    ///
    /// Fails on database errors and undecodable rows.
    pub async fn get(&self, download_id: Uuid) -> Result<Option<DownloadWithTorrent>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            download_columns!(),
            r#" FROM "Downloads" WHERE "DownloadId" = ?"#
        ))
        .bind(uuid_text(download_id))
        .fetch_optional(self.pool)
        .await?;
        self.with_torrent(row).await
    }

    /// The download of a torrent with this restricted link (compared
    /// exactly) and its torrent.
    ///
    /// # Errors
    ///
    /// Fails on database errors and undecodable rows.
    pub async fn find(&self, torrent_id: Uuid, path: &str) -> Result<Option<DownloadWithTorrent>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            download_columns!(),
            r#" FROM "Downloads" WHERE "TorrentId" = ? AND "Path" = ? ORDER BY rowid LIMIT 1"#
        ))
        .bind(uuid_text(torrent_id))
        .bind(path)
        .fetch_optional(self.pool)
        .await?;
        self.with_torrent(row).await
    }

    async fn with_torrent(&self, row: Option<SqliteRow>) -> Result<Option<DownloadWithTorrent>> {
        let Some(row) = row else {
            return Ok(None);
        };
        let download = from_row(&row)?;
        let torrent = torrents::row_by_id(self.pool, download.torrent_id).await?;
        Ok(torrent.map(|torrent| DownloadWithTorrent { download, torrent }))
    }

    /// Creates a download of `info.restricted_link` for the torrent, queued
    /// now, unless the link is blank, the torrent is missing or already has
    /// a download for this link.
    ///
    /// Never fails because of a concurrent insert of the same download or a
    /// concurrent deletion of the torrent; those report
    /// [`AddResult::AlreadyExists`] and [`AddResult::TorrentMissing`].
    ///
    /// # Errors
    ///
    /// Fails on other database errors.
    pub async fn try_add(&self, torrent_id: Uuid, info: &DownloadInfo) -> Result<AddResult> {
        if info.restricted_link.trim().is_empty() {
            debug!(%torrent_id, "skipped creating a download with a blank link");
            return Ok(AddResult::InvalidInput);
        }
        let now = timestamp_text(codec::now());
        let id = uuid_text(torrent_id);
        let inserted = sqlx::query(
            r#"INSERT INTO "Downloads" ("DownloadId", "TorrentId", "Path", "Link", "Added",
                   "DownloadQueued", "DownloadStarted", "DownloadFinished", "UnpackingQueued",
                   "UnpackingStarted", "UnpackingFinished", "Completed", "RetryCount", "Error",
                   "RemoteId", "FileName")
               SELECT ?1, ?2, ?3, NULL, ?4, ?4, NULL, NULL, NULL, NULL, NULL, NULL, 0, NULL,
                   NULL, ?5
               WHERE EXISTS (SELECT 1 FROM "Torrents" WHERE "TorrentId" = ?2 AND "Type" = 0)"#,
        )
        .bind(uuid_text(Uuid::new_v4()))
        .bind(&id)
        .bind(&info.restricted_link)
        .bind(&now)
        .bind(&info.file_name)
        .execute(self.pool)
        .await;
        match inserted {
            Ok(done) if done.rows_affected() == 0 => {
                debug!(%torrent_id, "skipped creating a download of a missing torrent");
                Ok(AddResult::TorrentMissing)
            }
            Ok(_) => Ok(AddResult::Added),
            Err(sqlx::Error::Database(error)) if is_duplicate_download(error.as_ref()) => {
                debug!(%torrent_id, "skipped creating a download that already exists");
                Ok(AddResult::AlreadyExists)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Stores the unrestricted link.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_link(&self, download_id: Uuid, link: &str) -> Result<()> {
        self.set(download_id, r#""Link" = ?"#, Some(link)).await
    }

    /// Stores the file name.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_file_name(&self, download_id: Uuid, file_name: &str) -> Result<()> {
        self.set(download_id, r#""FileName" = ?"#, Some(file_name))
            .await
    }

    /// Stores the restricted link.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_path(&self, download_id: Uuid, path: &str) -> Result<()> {
        self.set(download_id, r#""Path" = ?"#, Some(path)).await
    }

    /// Stores when the transfer started.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_download_started(&self, id: Uuid, at: Option<Timestamp>) -> Result<()> {
        self.set_time(id, r#""DownloadStarted" = ?"#, at).await
    }

    /// Stores when the transfer finished.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_download_finished(&self, id: Uuid, at: Option<Timestamp>) -> Result<()> {
        self.set_time(id, r#""DownloadFinished" = ?"#, at).await
    }

    /// Stores when unpacking was queued.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_unpacking_queued(&self, id: Uuid, at: Option<Timestamp>) -> Result<()> {
        self.set_time(id, r#""UnpackingQueued" = ?"#, at).await
    }

    /// Stores when unpacking started.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_unpacking_started(&self, id: Uuid, at: Option<Timestamp>) -> Result<()> {
        self.set_time(id, r#""UnpackingStarted" = ?"#, at).await
    }

    /// Stores when unpacking finished.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_unpacking_finished(&self, id: Uuid, at: Option<Timestamp>) -> Result<()> {
        self.set_time(id, r#""UnpackingFinished" = ?"#, at).await
    }

    /// Stores when the download was completed.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_completed(&self, id: Uuid, at: Option<Timestamp>) -> Result<()> {
        self.set_time(id, r#""Completed" = ?"#, at).await
    }

    /// Stores the error shown to the user.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_error(&self, download_id: Uuid, error: Option<&str>) -> Result<()> {
        self.set(download_id, r#""Error" = ?"#, error).await
    }

    /// Stores the retries consumed so far.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_retry_count(&self, download_id: Uuid, retry_count: i32) -> Result<()> {
        sqlx::query(r#"UPDATE "Downloads" SET "RetryCount" = ? WHERE "DownloadId" = ?"#)
            .bind(retry_count)
            .bind(uuid_text(download_id))
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Stores the identifier of the transfer in the download engine.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_remote_id(&self, download_id: Uuid, remote_id: &str) -> Result<()> {
        self.set(download_id, r#""RemoteId" = ?"#, Some(remote_id))
            .await
    }

    /// Deletes every download of a torrent.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn delete_for_torrent(&self, torrent_id: Uuid) -> Result<()> {
        sqlx::query(r#"DELETE FROM "Downloads" WHERE "TorrentId" = ?"#)
            .bind(uuid_text(torrent_id))
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Queues the download again from scratch: clears the unrestricted
    /// link, every progress timestamp, the error and the retry count, sets
    /// `added` to now and `download_queued` to `queued` (now when `None`).
    /// The restricted link, file name and remote id are kept.
    ///
    /// # Errors
    ///
    /// Fails with [`StoreError::DownloadNotFound`] when the download does
    /// not exist, and on database errors.
    pub async fn reset(&self, download_id: Uuid, queued: Option<Timestamp>) -> Result<()> {
        let now = codec::now();
        let updated = sqlx::query(
            r#"UPDATE "Downloads" SET "RetryCount" = 0, "Link" = NULL, "Added" = ?,
                   "DownloadQueued" = ?, "DownloadStarted" = NULL, "DownloadFinished" = NULL,
                   "UnpackingQueued" = NULL, "UnpackingStarted" = NULL,
                   "UnpackingFinished" = NULL, "Completed" = NULL, "Error" = NULL
               WHERE "DownloadId" = ?"#,
        )
        .bind(timestamp_text(now))
        .bind(timestamp_text(queued.unwrap_or(now)))
        .bind(uuid_text(download_id))
        .execute(self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::DownloadNotFound(download_id));
        }
        Ok(())
    }

    async fn set_time(
        &self,
        download_id: Uuid,
        assignment: &str,
        at: Option<Timestamp>,
    ) -> Result<()> {
        self.set(
            download_id,
            assignment,
            optional_timestamp_text(at).as_deref(),
        )
        .await
    }

    /// Runs `UPDATE "Downloads" SET <assignment> WHERE <download>` with one
    /// bound text value.
    async fn set(&self, download_id: Uuid, assignment: &str, value: Option<&str>) -> Result<()> {
        let sql = format!(r#"UPDATE "Downloads" SET {assignment} WHERE "DownloadId" = ?"#);
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(value)
            .bind(uuid_text(download_id))
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn is_duplicate_download(error: &dyn DatabaseError) -> bool {
    error.code().as_deref() == Some(UNIQUE_VIOLATION)
        || error
            .message()
            .contains("UNIQUE constraint failed: Downloads.TorrentId, Downloads.Path")
}

/// Decodes a row selected with [`download_columns`].
pub(crate) fn from_row(row: &SqliteRow) -> Result<Download> {
    Ok(Download {
        download_id: row::uuid(row, "DownloadId")?,
        torrent_id: row::uuid(row, "TorrentId")?,
        path: row::text(row, "Path")?,
        link: row::optional_text(row, "Link")?,
        added: row::timestamp(row, "Added")?,
        download_queued: row::optional_timestamp(row, "DownloadQueued")?,
        download_started: row::optional_timestamp(row, "DownloadStarted")?,
        download_finished: row::optional_timestamp(row, "DownloadFinished")?,
        unpacking_queued: row::optional_timestamp(row, "UnpackingQueued")?,
        unpacking_started: row::optional_timestamp(row, "UnpackingStarted")?,
        unpacking_finished: row::optional_timestamp(row, "UnpackingFinished")?,
        completed: row::optional_timestamp(row, "Completed")?,
        retry_count: row::int(row, "RetryCount")?,
        error: row::optional_text(row, "Error")?,
        remote_id: row::optional_text(row, "RemoteId")?,
        file_name: row::optional_text(row, "FileName")?,
    })
}

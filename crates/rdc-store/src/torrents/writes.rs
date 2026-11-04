//! Torrent writes. Every update touches only the columns it names, so
//! concurrent writers of different columns never overwrite each other, and
//! silently does nothing when the torrent does not exist.

use jiff::Timestamp;
use rdc_model::{DownloadClient, Torrent};
use sqlx::SqliteConnection;
use tracing::debug;
use uuid::Uuid;

use super::Torrents;
use crate::{
    codec::{self, optional_timestamp_text, timestamp_text, uuid_text},
    error::Result,
};

/// A torrent to add with [`Torrents::add`].
#[derive(Debug, Clone, Copy)]
pub struct NewTorrent<'a> {
    /// Provider-side id, when the torrent already exists at the provider.
    pub rd_id: Option<&'a str>,
    /// Info hash; stored lower-case.
    pub hash: &'a str,
    /// Magnet link, or base64 `.torrent` bytes when `is_file` is set;
    /// blank payloads are not stored.
    pub payload: Option<&'a str>,
    /// Whether the payload is a `.torrent` file.
    pub is_file: bool,
    /// Engine that transfers the files to the host.
    pub download_client: DownloadClient,
    /// Source of the per-torrent options: category, host download action,
    /// finished action and its delay, download action, minimum size,
    /// include/exclude expressions, manual files, priority, retry attempts,
    /// delete-on-error and lifetime minutes, provider status and name.
    pub template: &'a Torrent,
}

/// Whether [`Torrents::update_complete`] schedules a retry of the whole
/// torrent when it completes with an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOnError {
    /// Count a torrent retry and request it now, while attempts remain.
    Schedule,
    /// Leave the retry state alone.
    Skip,
}

fn is_blank(text: Option<&str>) -> bool {
    text.is_none_or(|text| text.trim().is_empty())
}

impl Torrents<'_> {
    /// Adds a torrent with a new id, added now, together with its payload,
    /// and returns it as stored.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn add(&self, new: &NewTorrent<'_>) -> Result<Torrent> {
        let template = new.template;
        let torrent = Torrent {
            torrent_id: Uuid::new_v4(),
            added: codec::now(),
            rd_id: new.rd_id.map(str::to_owned),
            hash: new.hash.to_lowercase(),
            is_file: new.is_file,
            download_client: new.download_client,
            payload: new
                .payload
                .filter(|payload| !is_blank(Some(payload)))
                .map(str::to_owned),
            category: template.category.clone(),
            host_download_action: template.host_download_action,
            finished_action_delay: template.finished_action_delay,
            download_action: template.download_action,
            finished_action: template.finished_action,
            download_min_size: template.download_min_size,
            include_regex: template.include_regex.clone(),
            exclude_regex: template.exclude_regex.clone(),
            download_manual_files: template.download_manual_files.clone(),
            priority: template.priority,
            torrent_retry_attempts: template.torrent_retry_attempts,
            download_retry_attempts: template.download_retry_attempts,
            delete_on_error: template.delete_on_error,
            lifetime: template.lifetime,
            rd_status: template.rd_status,
            rd_name: template.rd_name.clone(),
            ..Torrent::default()
        };
        let mut transaction = self.pool.begin().await?;
        insert(&mut transaction, &torrent).await?;
        if let Some(payload) = &torrent.payload {
            sqlx::query(r#"INSERT INTO "TorrentPayloads" ("TorrentId", "Content") VALUES (?, ?)"#)
                .bind(uuid_text(torrent.torrent_id))
                .bind(payload)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(torrent)
    }

    /// Stores the provider mirror of `torrent`: its provider kind and every
    /// `rd_*` field except the provider id.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_rd_data(&self, torrent: &Torrent) -> Result<()> {
        sqlx::query(
            r#"UPDATE "Torrents" SET "ClientKind" = ?, "RdName" = ?, "RdSize" = ?, "RdHost" = ?,
                   "RdSplit" = ?, "RdProgress" = ?, "RdStatus" = ?, "RdStatusRaw" = ?,
                   "RdAdded" = ?, "RdEnded" = ?, "RdSpeed" = ?, "RdSeeders" = ?, "RdFiles" = ?
               WHERE "TorrentId" = ? AND "Type" = 0"#,
        )
        .bind(torrent.client_kind.map(i32::from))
        .bind(&torrent.rd_name)
        .bind(torrent.rd_size)
        .bind(&torrent.rd_host)
        .bind(torrent.rd_split)
        .bind(torrent.rd_progress)
        .bind(torrent.rd_status.map(i32::from))
        .bind(&torrent.rd_status_raw)
        .bind(optional_timestamp_text(torrent.rd_added))
        .bind(optional_timestamp_text(torrent.rd_ended))
        .bind(torrent.rd_speed)
        .bind(torrent.rd_seeders)
        .bind(&torrent.rd_files)
        .bind(uuid_text(torrent.torrent_id))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Stores the user-editable options of `torrent`: download client, host
    /// download action, category, priority, download and torrent retry
    /// attempts, delete-on-error and lifetime minutes.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update(&self, torrent: &Torrent) -> Result<()> {
        sqlx::query(
            r#"UPDATE "Torrents" SET "DownloadClient" = ?, "HostDownloadAction" = ?,
                   "Category" = ?, "Priority" = ?, "DownloadRetryAttempts" = ?,
                   "TorrentRetryAttempts" = ?, "DeleteOnError" = ?, "Lifetime" = ?
               WHERE "TorrentId" = ? AND "Type" = 0"#,
        )
        .bind(i32::from(torrent.download_client))
        .bind(i32::from(torrent.host_download_action))
        .bind(&torrent.category)
        .bind(torrent.priority)
        .bind(torrent.download_retry_attempts)
        .bind(torrent.torrent_retry_attempts)
        .bind(torrent.delete_on_error)
        .bind(torrent.lifetime)
        .bind(uuid_text(torrent.torrent_id))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Stores the provider-side id.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_rd_id(&self, torrent_id: Uuid, rd_id: &str) -> Result<()> {
        self.set(torrent_id, r#""RdId" = ?"#, rd_id).await
    }

    /// Stores the info hash, lower-cased.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_hash(&self, torrent_id: Uuid, hash: &str) -> Result<()> {
        self.set(torrent_id, r#""Hash" = ?"#, hash.to_lowercase())
            .await
    }

    /// Stores the category.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_category(&self, torrent_id: Uuid, category: Option<&str>) -> Result<()> {
        self.set(torrent_id, r#""Category" = ?"#, category).await
    }

    /// Stores when the files were selected on the provider.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_files_selected(&self, torrent_id: Uuid, at: Timestamp) -> Result<()> {
        self.set(torrent_id, r#""FilesSelected" = ?"#, timestamp_text(at))
            .await
    }

    /// Stores the queue priority.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_priority(&self, torrent_id: Uuid, priority: Option<i32>) -> Result<()> {
        self.set(torrent_id, r#""Priority" = ?"#, priority).await
    }

    /// Stores the error shown to the user.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_error(&self, torrent_id: Uuid, error: &str) -> Result<()> {
        self.set(torrent_id, r#""Error" = ?"#, error).await
    }

    /// Stores the torrent retry request and the retries consumed so far.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_retry(
        &self,
        torrent_id: Uuid,
        retry: Option<Timestamp>,
        retry_count: i32,
    ) -> Result<()> {
        sqlx::query(
            r#"UPDATE "Torrents" SET "Retry" = ?, "RetryCount" = ?
               WHERE "TorrentId" = ? AND "Type" = 0"#,
        )
        .bind(optional_timestamp_text(retry))
        .bind(retry_count)
        .bind(uuid_text(torrent_id))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Marks the torrent completed at `completed` with `error`; `None` for
    /// both clears a previous completion.
    ///
    /// A missing or blank `error` is replaced by
    /// `"{failed}/{total} downloads failed with errors"` when some downloads
    /// carry an error. When the resulting error is not blank and `retry` is
    /// [`RetryOnError::Schedule`], one more torrent retry is counted and
    /// requested now, unless the torrent used up its retry attempts.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_complete(
        &self,
        torrent_id: Uuid,
        error: Option<&str>,
        completed: Option<Timestamp>,
        retry: RetryOnError,
    ) -> Result<()> {
        let id = uuid_text(torrent_id);
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut error = error.map(str::to_owned);
        if is_blank(error.as_deref()) {
            let errors: Vec<Option<String>> =
                sqlx::query_scalar(r#"SELECT "Error" FROM "Downloads" WHERE "TorrentId" = ?"#)
                    .bind(&id)
                    .fetch_all(&mut *transaction)
                    .await?;
            let failed = errors
                .iter()
                .filter(|error| !is_blank(error.as_deref()))
                .count();
            if failed > 0 {
                error = Some(format!(
                    "{failed}/{} downloads failed with errors",
                    errors.len()
                ));
            }
        }
        let schedule = retry == RetryOnError::Schedule && !is_blank(error.as_deref());
        sqlx::query(
            r#"UPDATE "Torrents" SET
                   "RetryCount" = CASE WHEN ?1 AND "RetryCount" < "TorrentRetryAttempts"
                       THEN "RetryCount" + 1 ELSE "RetryCount" END,
                   "Retry" = CASE WHEN ?1 AND "RetryCount" < "TorrentRetryAttempts"
                       THEN ?2 ELSE "Retry" END,
                   "Completed" = ?3, "Error" = ?4
               WHERE "TorrentId" = ?5 AND "Type" = 0"#,
        )
        .bind(schedule)
        .bind(timestamp_text(codec::now()))
        .bind(optional_timestamp_text(completed))
        .bind(error)
        .bind(&id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Deletes the torrent with its downloads and payload in one
    /// transaction; a missing torrent is not an error.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn delete(&self, torrent_id: Uuid) -> Result<()> {
        let id = uuid_text(torrent_id);
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let exists: Option<i64> =
            sqlx::query_scalar(r#"SELECT 1 FROM "Torrents" WHERE "TorrentId" = ? AND "Type" = 0"#)
                .bind(&id)
                .fetch_optional(&mut *transaction)
                .await?;
        if exists.is_none() {
            debug!(%torrent_id, "skipped deleting a torrent that does not exist");
            return Ok(());
        }
        for sql in [
            r#"DELETE FROM "Downloads" WHERE "TorrentId" = ?"#,
            r#"DELETE FROM "TorrentPayloads" WHERE "TorrentId" = ?"#,
            r#"DELETE FROM "Torrents" WHERE "TorrentId" = ?"#,
        ] {
            sqlx::query(sql)
                .bind(&id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Runs `UPDATE "Torrents" SET <assignment> WHERE <torrent>` with one
    /// bound value.
    async fn set<'value, T>(&self, torrent_id: Uuid, assignment: &str, value: T) -> Result<()>
    where
        T: sqlx::Encode<'value, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send + 'value,
    {
        let sql =
            format!(r#"UPDATE "Torrents" SET {assignment} WHERE "TorrentId" = ? AND "Type" = 0"#);
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(value)
            .bind(uuid_text(torrent_id))
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

/// Inserts every column of a new torrent row.
async fn insert(connection: &mut SqliteConnection, torrent: &Torrent) -> Result<()> {
    sqlx::query(concat!(
        r#"INSERT INTO "Torrents" ("#,
        torrent_columns!(),
        r#", "Type") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)"#
    ))
    .bind(uuid_text(torrent.torrent_id))
    .bind(&torrent.hash)
    .bind(&torrent.category)
    .bind(i32::from(torrent.download_action))
    .bind(i32::from(torrent.finished_action))
    .bind(torrent.finished_action_delay)
    .bind(i32::from(torrent.host_download_action))
    .bind(torrent.download_min_size)
    .bind(&torrent.include_regex)
    .bind(&torrent.exclude_regex)
    .bind(&torrent.download_manual_files)
    .bind(i32::from(torrent.download_client))
    .bind(timestamp_text(torrent.added))
    .bind(optional_timestamp_text(torrent.files_selected))
    .bind(optional_timestamp_text(torrent.completed))
    .bind(optional_timestamp_text(torrent.retry))
    .bind(torrent.is_file)
    .bind(torrent.priority)
    .bind(torrent.retry_count)
    .bind(torrent.download_retry_attempts)
    .bind(torrent.torrent_retry_attempts)
    .bind(torrent.delete_on_error)
    .bind(torrent.lifetime)
    .bind(&torrent.error)
    .bind(torrent.client_kind.map(i32::from))
    .bind(&torrent.rd_id)
    .bind(&torrent.rd_name)
    .bind(torrent.rd_size)
    .bind(&torrent.rd_host)
    .bind(torrent.rd_split)
    .bind(torrent.rd_progress)
    .bind(torrent.rd_status.map(i32::from))
    .bind(&torrent.rd_status_raw)
    .bind(optional_timestamp_text(torrent.rd_added))
    .bind(optional_timestamp_text(torrent.rd_ended))
    .bind(torrent.rd_speed)
    .bind(torrent.rd_seeders)
    .bind(&torrent.rd_files)
    .execute(connection)
    .await?;
    Ok(())
}

//! Torrent rows with their downloads and payloads.
//!
//! Only torrent rows (`Type = 0`) are visible; other row types of the
//! existing database layout are ignored by every operation.

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use rdc_model::{Download, Torrent};
use sqlx::{SqlitePool, sqlite::SqliteRow};
use uuid::Uuid;

use crate::{
    codec,
    downloads::{self, download_columns},
    error::Result,
    row,
};

/// Every column of `Torrents` but `Type`, in [`from_row`] order.
macro_rules! torrent_columns {
    () => {
        r#""TorrentId", "Hash", "Category", "DownloadAction", "FinishedAction",
        "FinishedActionDelay", "HostDownloadAction", "DownloadMinSize", "IncludeRegex",
        "ExcludeRegex", "DownloadManualFiles", "DownloadClient", "Added", "FilesSelected",
        "Completed", "Retry", "IsFile", "Priority", "RetryCount", "DownloadRetryAttempts",
        "TorrentRetryAttempts", "DeleteOnError", "Lifetime", "Error", "ClientKind", "RdId",
        "RdName", "RdSize", "RdHost", "RdSplit", "RdProgress", "RdStatus", "RdStatusRaw",
        "RdAdded", "RdEnded", "RdSpeed", "RdSeeders", "RdFiles""#
    };
}

mod writes;

pub use self::writes::{NewTorrent, RetryOnError};

/// Sort key of torrents without a priority.
const NO_PRIORITY: i32 = 9999;

/// Torrent operations; obtained from [`Store::torrents`](crate::Store::torrents).
#[derive(Debug, Clone, Copy)]
pub struct Torrents<'store> {
    pool: &'store SqlitePool,
}

impl<'store> Torrents<'store> {
    pub(crate) fn new(pool: &'store SqlitePool) -> Self {
        Self { pool }
    }

    /// Every torrent with its downloads but without its payload, ordered by
    /// priority (lower first, none last) and then by when it was added.
    ///
    /// # Errors
    ///
    /// Fails on database errors and undecodable rows.
    pub async fn list(&self) -> Result<Vec<Torrent>> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            torrent_columns!(),
            r#" FROM "Torrents" WHERE "Type" = 0 ORDER BY COALESCE("Priority", 9999), rowid"#
        ))
        .fetch_all(self.pool)
        .await?;
        let mut torrents = rows.iter().map(from_row).collect::<Result<Vec<_>>>()?;

        let rows = sqlx::query(concat!(
            "SELECT ",
            download_columns!(),
            r#" FROM "Downloads" WHERE "TorrentId" IN
                (SELECT "TorrentId" FROM "Torrents" WHERE "Type" = 0) ORDER BY rowid"#
        ))
        .fetch_all(self.pool)
        .await?;
        let mut by_torrent: HashMap<Uuid, Vec<Download>> = HashMap::new();
        for row in &rows {
            let download = downloads::from_row(row)?;
            by_torrent
                .entry(download.torrent_id)
                .or_default()
                .push(download);
        }
        for torrent in &mut torrents {
            torrent.downloads = by_torrent.remove(&torrent.torrent_id).unwrap_or_default();
        }

        torrents.sort_by_key(|torrent| (torrent.priority.unwrap_or(NO_PRIORITY), torrent.added));
        Ok(torrents)
    }

    /// The torrent with this id, with its downloads and payload.
    ///
    /// # Errors
    ///
    /// Fails on database errors and undecodable rows.
    pub async fn get(&self, torrent_id: Uuid) -> Result<Option<Torrent>> {
        let Some(mut torrent) = row_by_id(self.pool, torrent_id).await? else {
            return Ok(None);
        };
        torrent.downloads = self.downloads_of(torrent_id).await?;
        torrent.payload = self.payload(torrent_id).await?;
        Ok(Some(torrent))
    }

    /// The first torrent whose hash equals `hash` lower-cased, with its
    /// downloads but without its payload.
    ///
    /// Hashes are stored lower-case and compared exactly.
    ///
    /// # Errors
    ///
    /// Fails on database errors and undecodable rows.
    pub async fn find_by_hash(&self, hash: &str) -> Result<Option<Torrent>> {
        let row = sqlx::query(concat!(
            "SELECT ",
            torrent_columns!(),
            r#" FROM "Torrents" WHERE "Hash" = ? AND "Type" = 0 ORDER BY rowid LIMIT 1"#
        ))
        .bind(hash.to_lowercase())
        .fetch_optional(self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut torrent = from_row(&row)?;
        torrent.downloads = self.downloads_of(torrent.torrent_id).await?;
        Ok(Some(torrent))
    }

    /// The payload of a torrent: its magnet link, or the base64 `.torrent`
    /// file when [`Torrent::is_file`] is set.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn payload(&self, torrent_id: Uuid) -> Result<Option<String>> {
        let content = sqlx::query_scalar(
            r#"SELECT p."Content" FROM "TorrentPayloads" AS p
               JOIN "Torrents" AS t ON t."TorrentId" = p."TorrentId"
               WHERE p."TorrentId" = ? AND t."Type" = 0 LIMIT 1"#,
        )
        .bind(codec::uuid_text(torrent_id))
        .fetch_optional(self.pool)
        .await?;
        Ok(content)
    }

    async fn downloads_of(&self, torrent_id: Uuid) -> Result<Vec<Download>> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            download_columns!(),
            r#" FROM "Downloads" WHERE "TorrentId" = ? ORDER BY rowid"#
        ))
        .bind(codec::uuid_text(torrent_id))
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(downloads::from_row).collect()
    }
}

/// The visible torrent row with this id, without downloads and payload.
pub(crate) async fn row_by_id(pool: &SqlitePool, torrent_id: Uuid) -> Result<Option<Torrent>> {
    let row = sqlx::query(concat!(
        "SELECT ",
        torrent_columns!(),
        r#" FROM "Torrents" WHERE "TorrentId" = ? AND "Type" = 0"#
    ))
    .bind(codec::uuid_text(torrent_id))
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(from_row).transpose()
}

/// Decodes a row selected with [`torrent_columns`]; downloads and payload
/// are left empty.
fn from_row(row: &SqliteRow) -> Result<Torrent> {
    Ok(Torrent {
        torrent_id: row::uuid(row, "TorrentId")?,
        hash: row::text(row, "Hash")?,
        category: row::optional_text(row, "Category")?,
        download_action: row::enumeration(row, "DownloadAction")?,
        finished_action: row::enumeration(row, "FinishedAction")?,
        finished_action_delay: row::int(row, "FinishedActionDelay")?,
        host_download_action: row::enumeration(row, "HostDownloadAction")?,
        download_min_size: row::int(row, "DownloadMinSize")?,
        include_regex: row::optional_text(row, "IncludeRegex")?,
        exclude_regex: row::optional_text(row, "ExcludeRegex")?,
        download_manual_files: row::optional_text(row, "DownloadManualFiles")?,
        download_client: row::enumeration_or_default(row, "DownloadClient")?,
        added: row::timestamp(row, "Added")?,
        files_selected: row::optional_timestamp(row, "FilesSelected")?,
        completed: row::optional_timestamp(row, "Completed")?,
        retry: row::optional_timestamp(row, "Retry")?,
        is_file: row::flag(row, "IsFile")?,
        priority: row::optional_int(row, "Priority")?,
        retry_count: row::int(row, "RetryCount")?,
        download_retry_attempts: row::int(row, "DownloadRetryAttempts")?,
        torrent_retry_attempts: row::int(row, "TorrentRetryAttempts")?,
        delete_on_error: row::int(row, "DeleteOnError")?,
        lifetime: row::int(row, "Lifetime")?,
        error: row::optional_text(row, "Error")?,
        client_kind: row::optional_enumeration(row, "ClientKind")?,
        rd_id: row::optional_text(row, "RdId")?,
        rd_name: row::optional_text(row, "RdName")?,
        rd_size: row::optional_long(row, "RdSize")?,
        rd_host: row::optional_text(row, "RdHost")?,
        rd_split: row::optional_long(row, "RdSplit")?,
        rd_progress: row::optional_long(row, "RdProgress")?,
        rd_status: row::optional_enumeration(row, "RdStatus")?,
        rd_status_raw: row::optional_text(row, "RdStatusRaw")?,
        rd_added: row::optional_timestamp(row, "RdAdded")?,
        rd_ended: row::optional_timestamp(row, "RdEnded")?,
        rd_speed: row::optional_long(row, "RdSpeed")?,
        rd_seeders: row::optional_long(row, "RdSeeders")?,
        rd_files: row::optional_text(row, "RdFiles")?,
        downloads: Vec::new(),
        payload: None,
    })
}

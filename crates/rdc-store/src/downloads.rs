//! Download rows: one file of a torrent transferred to the host.

use rdc_model::Download;
use sqlx::sqlite::SqliteRow;

use crate::{error::Result, row};

/// Every column of `Downloads`, in [`from_row`] order.
macro_rules! download_columns {
    () => {
        r#""DownloadId", "TorrentId", "Path", "Link", "Added", "DownloadQueued", "DownloadStarted",
        "DownloadFinished", "UnpackingQueued", "UnpackingStarted", "UnpackingFinished",
        "Completed", "RetryCount", "Error", "RemoteId", "FileName""#
    };
}
pub(crate) use download_columns;

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

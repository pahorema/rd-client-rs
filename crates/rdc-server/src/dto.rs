//! JSON shapes and human-readable torrent status for the web interface.

use jiff::Timestamp;
use rdc_engine::{DownloadStats, Engine};
use rdc_model::{
    Download, DownloadClient, RemoteFile, Torrent, TorrentDownloadAction, TorrentFinishedAction,
    TorrentHostDownloadAction, TorrentStatus, format_size,
};
use serde::Serialize;
use uuid::Uuid;

/// Selects which collections are included in a torrent response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Variant {
    /// Torrent rows only: no downloads, files or payload.
    List,
    /// Everything, including provider files and the original payload.
    Detail,
}

/// A torrent as the web client receives it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TorrentDto {
    torrent_id: Uuid,
    hash: String,
    category: Option<String>,
    download_action: TorrentDownloadAction,
    finished_action: TorrentFinishedAction,
    finished_action_delay: i32,
    host_download_action: TorrentHostDownloadAction,
    download_min_size: i32,
    include_regex: Option<String>,
    exclude_regex: Option<String>,
    download_manual_files: Option<String>,
    download_client: DownloadClient,
    added: Timestamp,
    files_selected: Option<Timestamp>,
    completed: Option<Timestamp>,
    file_or_magnet: Option<String>,
    is_file: bool,
    priority: Option<i32>,
    retry_count: i32,
    download_retry_attempts: i32,
    torrent_retry_attempts: i32,
    delete_on_error: i32,
    lifetime: i32,
    error: Option<String>,
    rd_id: Option<String>,
    rd_name: Option<String>,
    rd_size: Option<i64>,
    rd_host: Option<String>,
    rd_split: Option<i64>,
    rd_progress: Option<i64>,
    rd_status: Option<TorrentStatus>,
    rd_status_raw: Option<String>,
    rd_added: Option<Timestamp>,
    rd_ended: Option<Timestamp>,
    rd_speed: Option<i64>,
    rd_seeders: Option<i64>,
    status_text: String,
    files_count: usize,
    downloads_count: usize,
    files: Vec<FileDto>,
    downloads: Vec<DownloadDto>,
}

/// A provider file of a torrent.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileDto {
    id: i64,
    path: String,
    bytes: i64,
    selected: bool,
    download_link: Option<String>,
}

impl From<RemoteFile> for FileDto {
    fn from(file: RemoteFile) -> Self {
        Self {
            id: file.id,
            path: file.path,
            bytes: file.bytes,
            selected: file.selected,
            download_link: file.download_link,
        }
    }
}

/// A host transfer with its live statistics.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadDto {
    download_id: Uuid,
    torrent_id: Uuid,
    path: String,
    link: Option<String>,
    added: Timestamp,
    download_queued: Option<Timestamp>,
    download_started: Option<Timestamp>,
    download_finished: Option<Timestamp>,
    unpacking_queued: Option<Timestamp>,
    unpacking_started: Option<Timestamp>,
    unpacking_finished: Option<Timestamp>,
    completed: Option<Timestamp>,
    retry_count: i32,
    error: Option<String>,
    bytes_total: u64,
    bytes_done: u64,
    speed: u64,
}

impl DownloadDto {
    fn new(download: Download, stats: DownloadStats) -> Self {
        Self {
            download_id: download.download_id,
            torrent_id: download.torrent_id,
            path: download.path,
            link: download.link,
            added: download.added,
            download_queued: download.download_queued,
            download_started: download.download_started,
            download_finished: download.download_finished,
            unpacking_queued: download.unpacking_queued,
            unpacking_started: download.unpacking_started,
            unpacking_finished: download.unpacking_finished,
            completed: download.completed,
            retry_count: download.retry_count,
            error: download.error,
            bytes_total: stats.bytes_total,
            bytes_done: stats.bytes_done,
            speed: stats.speed,
        }
    }
}

impl TorrentDto {
    /// Convert a persisted torrent to its UI representation without exposing internal fields.
    pub(crate) fn new(engine: &Engine, torrent: Torrent, variant: Variant) -> Self {
        let status_text = status_text(&torrent, |id| engine.download_stats(id));
        let provider_files = torrent.files();
        let files_count = provider_files.len();
        let downloads_count = torrent.downloads.len();
        let files = if variant == Variant::Detail {
            provider_files.into_iter().map(FileDto::from).collect()
        } else {
            Vec::new()
        };
        let downloads = if variant == Variant::List {
            Vec::new()
        } else {
            torrent
                .downloads
                .into_iter()
                .map(|download| {
                    let stats = engine.download_stats(download.download_id);
                    DownloadDto::new(download, stats)
                })
                .collect()
        };
        Self {
            torrent_id: torrent.torrent_id,
            hash: torrent.hash,
            category: torrent.category,
            download_action: torrent.download_action,
            finished_action: torrent.finished_action,
            finished_action_delay: torrent.finished_action_delay,
            host_download_action: torrent.host_download_action,
            download_min_size: torrent.download_min_size,
            include_regex: torrent.include_regex,
            exclude_regex: torrent.exclude_regex,
            download_manual_files: torrent.download_manual_files,
            download_client: torrent.download_client,
            added: torrent.added,
            files_selected: torrent.files_selected,
            completed: torrent.completed,
            file_or_magnet: torrent.payload.filter(|_| variant == Variant::Detail),
            is_file: torrent.is_file,
            priority: torrent.priority,
            retry_count: torrent.retry_count,
            download_retry_attempts: torrent.download_retry_attempts,
            torrent_retry_attempts: torrent.torrent_retry_attempts,
            delete_on_error: torrent.delete_on_error,
            lifetime: torrent.lifetime,
            error: torrent.error,
            rd_id: torrent.rd_id,
            rd_name: torrent.rd_name,
            rd_size: torrent.rd_size,
            rd_host: torrent.rd_host,
            rd_split: torrent.rd_split,
            rd_progress: torrent.rd_progress,
            rd_status: torrent.rd_status,
            rd_status_raw: torrent.rd_status_raw,
            rd_added: torrent.rd_added,
            rd_ended: torrent.rd_ended,
            rd_speed: torrent.rd_speed,
            rd_seeders: torrent.rd_seeders,
            status_text,
            files_count,
            downloads_count,
            files,
            downloads,
        }
    }
}

/// Compute the first applicable status, prioritizing errors and active host transfers.
#[expect(
    clippy::cast_precision_loss,
    reason = "UI percentages need floating-point division of byte totals"
)]
pub(crate) fn status_text(t: &Torrent, stats: impl Fn(Uuid) -> DownloadStats) -> String {
    if let Some(error) = t.error.as_deref().filter(|error| !error.trim().is_empty()) {
        return error.to_owned();
    }
    if !t.downloads.is_empty() {
        let total = t.downloads.len();
        let mut finished = 0;
        let mut downloaded = 0;
        let mut downloading = 0;
        let mut unpacked = 0;
        let mut unpacking = 0;
        let mut queued_unpack = 0;
        let mut queued_download = 0;
        let (mut done, mut size, mut speed) = (0_u64, 0_u64, 0_u64);
        let (mut unpack_done, mut unpack_size) = (0_u64, 0_u64);
        for d in &t.downloads {
            let s = stats(d.download_id);
            if d.completed.is_some() {
                finished += 1;
            }
            if d.download_finished.is_some() {
                downloaded += 1;
            }
            if d.unpacking_finished.is_some() {
                unpacked += 1;
            }
            if d.download_started.is_some() && d.download_finished.is_none() && s.bytes_done > 0 {
                downloading += 1;
                done = done.saturating_add(s.bytes_done);
                size = size.saturating_add(s.bytes_total);
                speed = speed.saturating_add(s.speed);
            }
            if d.unpacking_started.is_some() && d.unpacking_finished.is_none() && s.bytes_done > 0 {
                unpacking += 1;
                unpack_done = unpack_done.saturating_add(s.bytes_done);
                unpack_size = unpack_size.saturating_add(s.bytes_total);
            }
            if d.unpacking_queued.is_some() && d.unpacking_started.is_none() {
                queued_unpack += 1;
            }
            if d.download_started.is_none() && d.download_finished.is_none() {
                queued_download += 1;
            }
        }
        if finished == total {
            return "Finished".to_owned();
        }
        if downloading > 0 {
            let pct = if size > 0 {
                (done as f64 / size as f64) * 100.0
            } else {
                0.0
            };
            return format!(
                "Downloading file {}/{total} ({pct:.2}% - {}/s)",
                downloading + downloaded,
                format_size(Some(speed.try_into().unwrap_or(i64::MAX)))
            );
        }
        if unpacking > 0 {
            let pct = if unpack_size > 0 {
                (unpack_done as f64 / unpack_size as f64) * 100.0
            } else {
                0.0
            };
            return format!(
                "Extracting file {}/{total} ({pct:.2}%)",
                unpacking + unpacked
            );
        }
        if queued_unpack > 0 {
            return "Queued for unpacking".to_owned();
        }
        if queued_download > 0 {
            return "Queued for downloading".to_owned();
        }
        if unpacked > 0 {
            return "Files unpacked".to_owned();
        }
        if downloaded > 0 {
            return "Files downloaded to host".to_owned();
        }
    }
    if t.completed.is_some() {
        return "Finished".to_owned();
    }
    let speed = format_size(t.rd_speed);
    match t.rd_status {
        Some(TorrentStatus::Queued) => "Not Yet Added to Provider".to_owned(),
        Some(TorrentStatus::Downloading) if t.rd_seeders.is_some_and(|count| count < 1) => {
            "Torrent stalled".to_owned()
        }
        Some(TorrentStatus::Downloading) => format!(
            "Torrent downloading ({}% - {speed}/s)",
            t.rd_progress.map_or_else(String::new, |p| p.to_string())
        ),
        Some(TorrentStatus::Processing) => "Torrent processing".to_owned(),
        Some(TorrentStatus::WaitingForFileSelection) => {
            "Torrent waiting for file selection".to_owned()
        }
        Some(TorrentStatus::Error) => format!(
            "Torrent error: {}",
            t.rd_status_raw.as_deref().unwrap_or_default()
        ),
        Some(TorrentStatus::Finished) => "Torrent finished, waiting for download links".to_owned(),
        Some(TorrentStatus::Uploading) => "Torrent uploading".to_owned(),
        None => "Unknown status".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent(status: Option<TorrentStatus>) -> Torrent {
        Torrent {
            rd_status: status,
            ..Torrent::default()
        }
    }

    #[test]
    fn provider_status_and_errors_follow_precedence() {
        let mut t = torrent(Some(TorrentStatus::Downloading));
        t.rd_progress = Some(25);
        t.rd_speed = Some(2048);
        assert_eq!(
            status_text(&t, |_| DownloadStats::default()),
            "Torrent downloading (25% - 2 KB/s)"
        );
        t.rd_seeders = Some(0);
        assert_eq!(
            status_text(&t, |_| DownloadStats::default()),
            "Torrent stalled"
        );
        t.error = Some("provider failed".to_owned());
        assert_eq!(
            status_text(&t, |_| DownloadStats::default()),
            "provider failed"
        );
        t.error = None;
        t.rd_status = Some(TorrentStatus::Queued);
        assert_eq!(
            status_text(&t, |_| DownloadStats::default()),
            "Not Yet Added to Provider"
        );
        t.rd_status = Some(TorrentStatus::WaitingForFileSelection);
        assert_eq!(
            status_text(&t, |_| DownloadStats::default()),
            "Torrent waiting for file selection"
        );
        t.rd_status = Some(TorrentStatus::Error);
        t.rd_status_raw = Some("remote error".to_owned());
        assert_eq!(
            status_text(&t, |_| DownloadStats::default()),
            "Torrent error: remote error"
        );
        t.rd_status = None;
        assert_eq!(
            status_text(&t, |_| DownloadStats::default()),
            "Unknown status"
        );
    }

    #[test]
    fn host_transfer_status_prioritizes_progress_and_queueing() {
        let time = Timestamp::now();
        let mut t = torrent(Some(TorrentStatus::Finished));
        t.downloads.push(Download {
            download_started: Some(time),
            ..Download::default()
        });
        let active = DownloadStats {
            bytes_total: 100,
            bytes_done: 25,
            speed: 2048,
        };
        assert_eq!(
            status_text(&t, |_| active),
            "Downloading file 1/1 (25.00% - 2 KB/s)"
        );
        t.downloads[0].download_finished = Some(time);
        t.downloads[0].unpacking_started = Some(time);
        assert_eq!(status_text(&t, |_| active), "Extracting file 1/1 (25.00%)");
        t.downloads[0].unpacking_started = None;
        t.downloads[0].unpacking_queued = Some(time);
        assert_eq!(
            status_text(&t, |_| DownloadStats::default()),
            "Queued for unpacking"
        );
        t.downloads[0].unpacking_queued = None;
        assert_eq!(
            status_text(&t, |_| DownloadStats::default()),
            "Files downloaded to host"
        );
        t.downloads[0].completed = Some(time);
        assert_eq!(status_text(&t, |_| DownloadStats::default()), "Finished");
    }
}

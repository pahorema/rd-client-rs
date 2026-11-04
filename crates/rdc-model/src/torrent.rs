//! Torrents managed by rd-client.

use jiff::Timestamp;
use uuid::Uuid;

use crate::{
    Download, DownloadClient, Provider, RemoteFile, TorrentDownloadAction, TorrentFinishedAction,
    TorrentHostDownloadAction, TorrentStatus,
};

/// A torrent managed by rd-client (a row of the `Torrents` table).
///
/// The first group of fields is the per-torrent configuration copied from
/// the defaults the torrent was added with, then the local state, then the
/// `rd_*` mirror of the provider's view written by the provider poller.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Torrent {
    /// Primary key.
    pub torrent_id: Uuid,
    /// Lower-case info hash.
    pub hash: String,
    /// Category; also a sub-directory of the download path.
    pub category: Option<String>,
    /// Which files get selected on the provider.
    pub download_action: TorrentDownloadAction,
    /// What to remove once every file reached the host.
    pub finished_action: TorrentFinishedAction,
    /// Minutes to wait before running the finished action.
    pub finished_action_delay: i32,
    /// Whether provider files are transferred to the host.
    pub host_download_action: TorrentHostDownloadAction,
    /// Files of at most this many MiB are skipped (0 disables the rule).
    pub download_min_size: i32,
    /// Only files whose path matches this regular expression are kept.
    pub include_regex: Option<String>,
    /// Files whose path matches this regular expression are skipped
    /// (ignored when [`include_regex`](Self::include_regex) is set).
    pub exclude_regex: Option<String>,
    /// Comma-separated path suffixes of manually selected files.
    pub download_manual_files: Option<String>,
    /// Engine that transfers the files to the host.
    pub download_client: DownloadClient,
    /// When the torrent was added.
    pub added: Timestamp,
    /// When the files were selected on the provider.
    pub files_selected: Option<Timestamp>,
    /// When the torrent was completed (successfully or not).
    pub completed: Option<Timestamp>,
    /// When a retry of the whole torrent was requested.
    pub retry: Option<Timestamp>,
    /// Whether the payload is a `.torrent` file (base64) rather than a magnet.
    pub is_file: bool,
    /// Queue priority, lower first; `None` sorts last.
    pub priority: Option<i32>,
    /// Torrent retries consumed so far.
    pub retry_count: i32,
    /// Retries allowed per download.
    pub download_retry_attempts: i32,
    /// Retries allowed for the whole torrent.
    pub torrent_retry_attempts: i32,
    /// Minutes in error before the torrent is deleted (0 disables).
    pub delete_on_error: i32,
    /// Maximum lifetime in minutes (0 disables).
    pub lifetime: i32,
    /// Last error shown to the user.
    pub error: Option<String>,
    /// Provider that owns the `rd_*` fields.
    pub client_kind: Option<Provider>,
    /// Provider-side torrent id.
    pub rd_id: Option<String>,
    /// Torrent name reported by the provider.
    pub rd_name: Option<String>,
    /// Total size in bytes reported by the provider.
    pub rd_size: Option<i64>,
    /// Provider host information.
    pub rd_host: Option<String>,
    /// Provider split size.
    pub rd_split: Option<i64>,
    /// Provider progress in percent.
    pub rd_progress: Option<i64>,
    /// Normalised provider status; `None` until first reported.
    pub rd_status: Option<TorrentStatus>,
    /// Status text exactly as reported by the provider.
    pub rd_status_raw: Option<String>,
    /// When the provider added the torrent.
    pub rd_added: Option<Timestamp>,
    /// When the provider finished the torrent.
    pub rd_ended: Option<Timestamp>,
    /// Provider transfer speed in bytes per second.
    pub rd_speed: Option<i64>,
    /// Seeders reported by the provider.
    pub rd_seeders: Option<i64>,
    /// Provider file list in its stored JSON form; see [`Torrent::files`].
    pub rd_files: Option<String>,
    /// Downloads of this torrent (not a column).
    pub downloads: Vec<Download>,
    /// Magnet link, or base64 `.torrent` bytes when
    /// [`is_file`](Self::is_file) is set (stored in `TorrentPayloads`).
    pub payload: Option<String>,
}

impl Torrent {
    /// The provider file list decoded from [`rd_files`](Self::rd_files).
    ///
    /// Never fails: a missing, blank or undecodable list is empty (see
    /// [`RemoteFile::list_from_json`]).
    pub fn files(&self) -> Vec<RemoteFile> {
        self.rd_files
            .as_deref()
            .map(RemoteFile::list_from_json)
            .unwrap_or_default()
    }

    /// Replaces [`rd_files`](Self::rd_files) with the stored JSON form of
    /// `files`.
    pub fn set_files(&mut self, files: &[RemoteFile]) {
        self.rd_files = Some(RemoteFile::list_to_json(files));
    }

    /// The manually selected path suffixes.
    ///
    /// A missing or blank list is empty; otherwise the text is split on `,`
    /// without trimming and without dropping empty entries.
    pub fn manual_files(&self) -> Vec<&str> {
        match self.download_manual_files.as_deref() {
            Some(list) if !list.trim().is_empty() => list.split(',').collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_empty_without_a_list() {
        let torrent = Torrent::default();
        assert_eq!(torrent.files(), Vec::new());
    }

    #[test]
    fn files_tolerate_invalid_json() {
        let torrent = Torrent {
            rd_files: Some("{broken".to_owned()),
            ..Torrent::default()
        };
        assert_eq!(torrent.files(), Vec::new());
    }

    #[test]
    fn set_files_round_trips() {
        let files = vec![RemoteFile {
            id: 3,
            path: "dir/file.mkv".to_owned(),
            bytes: 42,
            selected: true,
            download_link: None,
        }];
        let mut torrent = Torrent::default();
        torrent.set_files(&files);
        assert_eq!(
            torrent.rd_files.as_deref(),
            Some(
                r#"[{"Id":3,"Path":"dir/file.mkv","Bytes":42,"Selected":true,"DownloadLink":null}]"#
            )
        );
        assert_eq!(torrent.files(), files);
    }

    #[test]
    fn manual_files_split_on_commas_without_trimming() {
        let torrent = Torrent {
            download_manual_files: Some("a.mkv, b.mkv,,c.srt".to_owned()),
            ..Torrent::default()
        };
        assert_eq!(torrent.manual_files(), ["a.mkv", " b.mkv", "", "c.srt"]);
    }

    #[test]
    fn manual_files_are_empty_when_blank() {
        for list in [None, Some(""), Some("  ")] {
            let torrent = Torrent {
                download_manual_files: list.map(str::to_owned),
                ..Torrent::default()
            };
            assert_eq!(torrent.manual_files(), Vec::<&str>::new(), "list {list:?}");
        }
    }
}

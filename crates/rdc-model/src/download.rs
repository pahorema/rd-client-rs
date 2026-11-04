//! Files of a torrent being transferred to the host.

use jiff::Timestamp;
use uuid::Uuid;

/// One file of a torrent transferred from the provider to the host
/// (a row of the `Downloads` table).
///
/// The timestamps record the life cycle: queued, started and finished for
/// the transfer, the same three for unpacking, then completed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Download {
    /// Primary key.
    pub download_id: Uuid,
    /// The torrent this download belongs to.
    pub torrent_id: Uuid,
    /// The provider's restricted link; unique per torrent.
    pub path: String,
    /// The unrestricted link, set once the restricted link was unlocked.
    pub link: Option<String>,
    /// When the row was created.
    pub added: Timestamp,
    /// When the transfer was queued.
    pub download_queued: Option<Timestamp>,
    /// When the transfer started.
    pub download_started: Option<Timestamp>,
    /// When the transfer finished.
    pub download_finished: Option<Timestamp>,
    /// When unpacking was queued.
    pub unpacking_queued: Option<Timestamp>,
    /// When unpacking started.
    pub unpacking_started: Option<Timestamp>,
    /// When unpacking finished.
    pub unpacking_finished: Option<Timestamp>,
    /// When the download was completed (successfully or not).
    pub completed: Option<Timestamp>,
    /// Retries consumed so far.
    pub retry_count: i32,
    /// Last error shown to the user.
    pub error: Option<String>,
    /// Identifier of the transfer in the download engine (aria2 gid,
    /// Download Station task id, symlink source, ...).
    pub remote_id: Option<String>,
    /// File name reported by the provider, without directories.
    pub file_name: Option<String>,
}

/// A downloadable file reported by the provider for a torrent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DownloadInfo {
    /// File name without directories; `None` when the provider does not
    /// report it and it has to be derived from the unrestricted link.
    pub file_name: Option<String>,
    /// The provider's restricted link, stored as [`Download::path`].
    pub restricted_link: String,
}

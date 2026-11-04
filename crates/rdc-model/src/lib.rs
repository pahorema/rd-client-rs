//! Domain types shared by every rd-client component: torrents, downloads,
//! provider files, the settings catalogue and the rules that decide which
//! files get downloaded.

mod download;
mod enums;
mod remote_file;
mod torrent;

pub use download::{Download, DownloadInfo};
pub use enums::{
    AuthenticationType, DownloadClient, DownloadClientLogLevel, LogLevel, ParseEnumError, Provider,
    TorrentDownloadAction, TorrentFinishedAction, TorrentHostDownloadAction, TorrentStatus,
    UnknownValueError,
};
pub use remote_file::RemoteFile;
pub use torrent::Torrent;

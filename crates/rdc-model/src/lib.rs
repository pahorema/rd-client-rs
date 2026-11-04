//! Domain types shared by every rd-client component: torrents, downloads,
//! provider files, the settings catalogue and the rules that decide which
//! files get downloaded.

mod download;
mod enums;
mod filter;
mod remote_file;
mod size;
mod torrent;

pub mod paths;
pub mod settings;

pub use download::{Download, DownloadInfo};
pub use enums::{
    AuthenticationType, DownloadClient, DownloadClientLogLevel, LogLevel, ParseEnumError, Provider,
    TorrentDownloadAction, TorrentFinishedAction, TorrentHostDownloadAction, TorrentStatus,
    UnknownValueError,
};
pub use filter::{FileFilter, FilterError};
pub use remote_file::RemoteFile;
pub use size::format_size;
pub use torrent::Torrent;

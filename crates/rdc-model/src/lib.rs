//! Domain types shared by every rd-client component: torrents, downloads,
//! provider files, the settings catalogue and the rules that decide which
//! files get downloaded.

mod enums;

pub use enums::{
    AuthenticationType, DownloadClient, DownloadClientLogLevel, LogLevel, ParseEnumError, Provider,
    TorrentDownloadAction, TorrentFinishedAction, TorrentHostDownloadAction, TorrentStatus,
    UnknownValueError,
};

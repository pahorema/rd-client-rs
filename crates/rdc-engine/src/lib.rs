//! Public torrent engine contracts and independent monitoring utilities.

mod error;
mod options;
mod types;

pub use error::EngineError;
pub use options::{
    DownloadFactory, DownloadRequest, EngineOptions, ProviderFactory, SettingsListener,
};
pub use types::{
    DeleteOptions, DownloadStats, EngineEvent, Profile, RateLimitStatus, TorrentTemplate,
    TorrentUpdate, WaitOutcome,
};

pub mod completion;
pub mod copy_added;
pub mod disk_space;
pub mod tracker_list;
pub mod update_check;
pub mod watch_folder;

//! The torrent lifecycle of rd-client: adding torrents to the debrid
//! provider, selecting files, downloading and unpacking them on the host,
//! completing, retrying and cleaning up, plus the background jobs that keep
//! it running.
//!
//! [`Engine::start`] repairs interrupted work, starts the serialized lifecycle tick and
//! independent monitoring jobs. Provider queue dispatch is shared by adds, polling and
//! the tick, so provider limits and cooldowns have one mutation path.

mod background;
mod core;
mod diagnostics;
mod download_runtime;
mod error;
mod finished;
mod operations;
mod options;
mod registry;
mod startup;
mod sync;
mod tick;
mod types;
mod unpack_runtime;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use core::Engine;
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

#[cfg(test)]
mod core_tests;
#[cfg(test)]
mod test_download;

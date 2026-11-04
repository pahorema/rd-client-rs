//! Torrent metadata, exact info hashes, magnet links and tracker enrichment.
//!
//! Parsing is bounded and performs no network or filesystem I/O. Tracker fetching,
//! caching and fail-open handling of fetch failures belong to the calling engine.

pub mod bencode;
mod enrich;
mod magnet;
mod torrent;

pub use enrich::{EnrichTorrentError, enrich_magnet, enrich_torrent};
pub use magnet::{MagnetLink, ParseMagnetError};
pub use torrent::{ParseTorrentError, TorrentEntry, TorrentFile};

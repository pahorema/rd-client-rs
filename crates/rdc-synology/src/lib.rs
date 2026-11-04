//! Client for the Synology DSM Web API, limited to what a Download Station downloader needs.
//!
//! It covers API discovery, login and logout, Download Station tasks and server
//! configuration, and File Station folder creation. It only speaks the protocol and knows
//! nothing about torrents.

mod error;

pub use error::Error;

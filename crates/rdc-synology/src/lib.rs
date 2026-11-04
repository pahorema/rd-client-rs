//! Client for the Synology DSM Web API, limited to what a Download Station downloader needs.
//!
//! It covers API discovery, login and logout, Download Station tasks and server
//! configuration, and File Station folder creation. It only speaks the protocol and knows
//! nothing about torrents.

mod api;
mod client;
mod codes;
mod error;

pub use client::{Client, DEFAULT_TIMEOUT};
pub use error::{ApiError, Error, ErrorDetail, SESSION_ERROR_CODES};

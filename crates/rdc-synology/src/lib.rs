//! Client for the Synology DSM Web API, limited to what a Download Station downloader needs.
//!
//! It covers API discovery, login and logout, Download Station tasks and server
//! configuration, and File Station folder creation. It only speaks the protocol and knows
//! nothing about torrents.
//!
//! ```no_run
//! # async fn run() -> Result<(), rdc_synology::Error> {
//! let client = rdc_synology::Client::connect("http://nas:5000").await?;
//! let session = client.login("downloader", "secret").await?;
//! session.logout().await?;
//! # Ok(())
//! # }
//! ```

mod api;
mod client;
mod codes;
mod config;
mod error;
mod session;
mod task;
mod wire;

pub use client::{Client, DEFAULT_TIMEOUT};
pub use config::ServerConfig;
pub use error::{ApiError, Error, ErrorDetail, SESSION_ERROR_CODES};
pub use session::Session;
pub use task::{StatusExtra, Task, TaskAdditional, TaskDetail, TaskFile, TaskStatus, TaskTransfer};

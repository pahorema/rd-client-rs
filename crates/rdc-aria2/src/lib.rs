//! Client for the aria2 JSON-RPC interface.
//!
//! [`Client`] talks JSON-RPC 2.0 over HTTP `POST` to an aria2 RPC endpoint (a URL ending in
//! `/jsonrpc`). It only speaks the protocol: it knows nothing about torrents or about where
//! the downloaded files end up.
//!
//! ```no_run
//! # async fn run() -> Result<(), rdc_aria2::Error> {
//! let client = rdc_aria2::Client::builder("http://127.0.0.1:6800/jsonrpc")
//!     .secret("mysecret123")
//!     .build()?;
//! let version = client.version().await?;
//! assert!(!version.version.is_empty());
//! # Ok(())
//! # }
//! ```

mod client;
mod error;

pub use client::{Client, ClientBuilder, DEFAULT_TIMEOUT, Version};
pub use error::Error;

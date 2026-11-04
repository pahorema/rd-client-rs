//! Client for the aria2 JSON-RPC interface.
//!
//! [`Client`] talks JSON-RPC 2.0 over HTTP `POST` to an aria2 RPC endpoint (a URL ending in
//! `/jsonrpc`). It only speaks the protocol: it knows nothing about torrents or about where
//! the downloaded files end up.

mod error;

pub use error::Error;

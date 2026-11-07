//! HTTP application plumbing and process configuration for rd-client.
#![recursion_limit = "256"]

mod accounts;
pub mod app;
pub mod auth;
pub mod config;
pub mod http;
pub mod logging;
mod preferences;
pub mod qbittorrent;

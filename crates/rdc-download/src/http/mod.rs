//! The internal HTTP engine.

mod config;
mod connection;
mod error;
mod transfer;

pub use self::config::InternalConfig;

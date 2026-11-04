//! Clients for the supported debrid services behind one [`DebridProvider`] trait,
//! plus the HTTP and rate-limit plumbing they share.

pub mod error;
pub mod http;
pub mod provider;
pub mod rate_limit;

pub use error::DebridError;
pub use http::{ProviderHttp, RetryPolicy};
pub use provider::{AvailableFile, DebridProvider, DebridUser, ProviderConfig, RemoteTorrent};
pub use rate_limit::RateLimitCoordinator;

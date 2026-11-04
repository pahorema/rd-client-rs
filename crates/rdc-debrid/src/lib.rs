//! Clients for the supported debrid services behind one [`DebridProvider`] trait,
//! plus the HTTP and rate-limit plumbing they share.

pub mod debrid_link;
mod endpoint;
pub mod error;
pub mod highway;
pub mod http;
mod json;
pub mod premiumize;
pub mod provider;
pub mod rate_limit;
pub mod real_debrid;
pub mod torbox;

pub use debrid_link::DebridLink;
pub use error::DebridError;
pub use highway::{Highway, is_spool_id};
pub use http::{ProviderHttp, RetryPolicy};
pub use premiumize::Premiumize;
pub use provider::{AvailableFile, DebridProvider, DebridUser, ProviderConfig, RemoteTorrent};
pub use rate_limit::RateLimitCoordinator;
pub use real_debrid::RealDebrid;
pub use torbox::TorBox;

//! Clients for the supported debrid services behind one provider trait, plus
//! the HTTP and rate-limit plumbing they share.

pub mod error;
pub mod rate_limit;

pub use error::DebridError;
pub use rate_limit::RateLimitCoordinator;

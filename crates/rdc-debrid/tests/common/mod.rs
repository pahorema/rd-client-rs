//! Helpers shared by the provider integration tests.

use std::sync::Arc;
use std::time::Duration;

use rdc_debrid::{ProviderConfig, ProviderHttp, RateLimitCoordinator, RetryPolicy};

/// API key used by every test.
pub(crate) const API_KEY: &str = "test-api-key";

/// Provider settings with [`API_KEY`].
pub(crate) fn config() -> ProviderConfig {
    ProviderConfig {
        api_key: API_KEY.to_owned(),
        api_hostname: None,
        timeout: Duration::from_secs(5),
        prefer_zipped_downloads: false,
    }
}

/// An HTTP client without transport retries and with its own coordinator.
pub(crate) fn http() -> ProviderHttp {
    ProviderHttp::new(
        Duration::from_secs(5),
        Arc::new(RateLimitCoordinator::new()),
    )
    .expect("client builds")
    .with_retry_policy(RetryPolicy {
        max_retries: 0,
        base_delay: Duration::ZERO,
    })
}

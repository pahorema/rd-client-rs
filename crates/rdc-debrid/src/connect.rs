//! Construction of the configured provider.

use std::sync::Arc;

use rdc_model::Provider;

use crate::{
    AllDebrid, DebridLink, DebridProvider, Highway, Premiumize, ProviderConfig, ProviderHttp,
    RealDebrid, TorBox,
};

/// Creates the client for `kind`.
///
/// The client is meant to live as long as the configuration does: some
/// providers keep listing state between calls.
pub fn connect(
    kind: Provider,
    config: &ProviderConfig,
    http: ProviderHttp,
) -> Arc<dyn DebridProvider> {
    match kind {
        Provider::RealDebrid => Arc::new(RealDebrid::new(config, http)),
        Provider::AllDebrid => Arc::new(AllDebrid::new(config, http)),
        Provider::Premiumize => Arc::new(Premiumize::new(config, http)),
        Provider::TorBox => Arc::new(TorBox::new(config, http)),
        Provider::DebridLink => Arc::new(DebridLink::new(config, http)),
        Provider::Highway => Arc::new(Highway::new(config, http)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::RateLimitCoordinator;

    #[test]
    fn every_provider_kind_gets_its_own_client() {
        let config = ProviderConfig {
            api_key: "key".to_owned(),
            api_hostname: None,
            timeout: Duration::from_secs(10),
            prefer_zipped_downloads: false,
        };
        let http = ProviderHttp::new(
            Duration::from_secs(10),
            Arc::new(RateLimitCoordinator::new()),
        )
        .expect("client builds");

        for kind in Provider::ALL {
            assert_eq!(connect(*kind, &config, http.clone()).kind(), *kind);
        }
    }
}

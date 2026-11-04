//! Construction hooks for applications and deterministic tests.

use crate::EngineError;
use async_trait::async_trait;
use rdc_debrid::{DebridProvider, ProviderConfig, ProviderHttp};
use rdc_download::{DownloadEngine, InternalConfig};
use rdc_model::{Download, Provider, Torrent, settings::Settings};
use std::{fmt, path::PathBuf, sync::Arc};
use tokio::sync::watch;

/// Receives complete settings snapshots after startup and every settings update.
pub type SettingsListener = Arc<dyn Fn(Arc<Settings>) + Send + Sync>;
/// Constructs the selected provider with the engine's shared cooldown coordinator.
pub type ProviderFactory = Arc<
    dyn Fn(Provider, &ProviderConfig, ProviderHttp) -> Result<Arc<dyn DebridProvider>, EngineError>
        + Send
        + Sync,
>;

/// Inputs supplied to an injected host downloader constructor.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    /// Download row being started or adopted.
    pub download: Download,
    /// Owning torrent and provider metadata.
    pub torrent: Torrent,
    /// Settings snapshot for remote downloader configuration.
    pub settings: Arc<Settings>,
    /// Sanitized local output filename.
    pub local_path: PathBuf,
    /// Sanitized path relative to the configured download root.
    pub relative_path: String,
    /// Live internal downloader configuration.
    pub internal_config: watch::Receiver<InternalConfig>,
}

/// Injection boundary used when starting or adopting a host transfer.
#[async_trait]
pub trait DownloadFactory: fmt::Debug + Send + Sync {
    /// Constructs an engine; the caller starts it and persists its remote id.
    async fn create(
        &self,
        request: DownloadRequest,
    ) -> Result<Arc<dyn DownloadEngine>, EngineError>;
}

/// Runtime integrations, all optional except the default provider constructor.
#[derive(Clone)]
pub struct EngineOptions {
    /// Applies live log-level and secret-redaction settings in the host application.
    pub settings_listener: Option<SettingsListener>,
    /// Provider constructor; defaults to the configured network provider.
    pub provider_factory: ProviderFactory,
    /// Overrides construction of host download engines.
    pub download_factory: Option<Arc<dyn DownloadFactory>>,
    /// Runs background monitoring and polling; disable for manually driven tests.
    pub background_jobs: bool,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            settings_listener: None,
            provider_factory: Arc::new(|kind, config, http| {
                Ok(rdc_debrid::connect(kind, config, http))
            }),
            download_factory: None,
            background_jobs: true,
        }
    }
}

impl fmt::Debug for EngineOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineOptions")
            .field("settings_listener", &self.settings_listener.is_some())
            .field("provider_factory", &"configured")
            .field("download_factory", &self.download_factory)
            .field("background_jobs", &self.background_jobs)
            .finish()
    }
}

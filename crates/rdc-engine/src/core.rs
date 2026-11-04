//! Engine ownership, settings publication and shared runtime state.

use crate::{
    DownloadFactory, DownloadStats, EngineError, EngineEvent, EngineOptions, Profile,
    RateLimitStatus,
    disk_space::{DiskSpaceConfig, DiskSpaceStatus},
    registry::Registry,
    tracker_list::TrackerList,
    update_check::UpdateChecker,
    watch_folder::WatchFolderConfig,
};
use rdc_debrid::{DebridError, DebridProvider, ProviderConfig, ProviderHttp, RateLimitCoordinator};
use rdc_download::InternalConfig;
use rdc_model::settings::Settings;
use rdc_store::Store;
use std::{
    fmt,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Mutex, broadcast, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Torrent operations and background monitoring, sharing one runtime across cheap clones.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("shutdown", &self.inner.cancel.is_cancelled())
            .finish_non_exhaustive()
    }
}

pub(crate) struct Inner {
    pub store: Store,
    pub settings: RwLock<Arc<Settings>>,
    pub provider: RwLock<Arc<dyn DebridProvider>>,
    pub provider_lock: Mutex<()>,
    pub reset_lock: Mutex<()>,
    pub coordinator: Arc<RateLimitCoordinator>,
    pub cancel: CancellationToken,
    pub trackers: TrackerList,
    pub registry: Registry,
    pub events: broadcast::Sender<EngineEvent>,
    pub ui_connected: AtomicBool,
    pub disk_status: RwLock<Option<DiskSpaceStatus>>,
    pub low_disk: AtomicBool,
    pub rate_observed: std::sync::Mutex<Option<jiff::Timestamp>>,
    pub updater: UpdateChecker,
    options: EngineOptions,
    settings_lock: Mutex<()>,
    jobs: Mutex<Vec<JoinHandle<()>>>,
    pub internal_config: watch::Sender<InternalConfig>,
    pub disk_config: watch::Sender<DiskSpaceConfig>,
    pub watch_config: watch::Sender<WatchFolderConfig>,
}

impl Engine {
    /// Seeds persisted settings, builds the provider and starts configured background jobs.
    ///
    /// The caller must have an active Tokio runtime. Startup repair and host processing
    /// are separate from the facade's settings and provider-monitoring lifecycle.
    pub async fn start(store: Store, options: EngineOptions) -> Result<Self, EngineError> {
        store.settings().seed().await?;
        let settings = Arc::new(store.settings().load().await?);
        let coordinator = Arc::new(RateLimitCoordinator::new());
        let provider = build_provider(&settings, &options, &coordinator)?;
        let (events, _) = broadcast::channel(128);
        let (internal_config, _) = watch::channel(internal_config(&settings));
        let (disk_config, _) = watch::channel(disk_config(&settings));
        let (watch_config, _) = watch::channel(watch_config(&settings));
        let engine = Self {
            inner: Arc::new(Inner {
                store,
                settings: RwLock::new(settings.clone()),
                provider: RwLock::new(provider),
                provider_lock: Mutex::new(()),
                reset_lock: Mutex::new(()),
                coordinator,
                cancel: CancellationToken::new(),
                trackers: TrackerList::new()?,
                registry: Registry::default(),
                events,
                ui_connected: AtomicBool::new(false),
                disk_status: RwLock::new(None),
                low_disk: AtomicBool::new(false),
                updater: UpdateChecker::new().map_err(|e| EngineError::Message(e.to_string()))?,
                rate_observed: std::sync::Mutex::new(None),
                options,
                settings_lock: Mutex::new(()),
                jobs: Mutex::new(Vec::new()),
                internal_config,
                disk_config,
                watch_config,
            }),
        };
        engine.notify_settings(settings);
        if engine.inner.options.background_jobs {
            *engine.inner.jobs.lock().await = crate::background::spawn(&engine);
        }
        Ok(engine)
    }
    /// Stops background jobs, then waits for active transfers and extractions to stop.
    ///
    /// All clones observe shutdown; repeated or simultaneous calls are safe. The store
    /// remains open because the host application may still need its user/settings APIs.
    pub async fn shutdown(&self) {
        self.inner.cancel.cancel();
        let mut jobs = self.inner.jobs.lock().await;
        for job in jobs.drain(..) {
            if let Err(error) = job.await {
                tracing::error!(%error, "engine background job stopped unexpectedly");
            }
        }
        self.inner.registry.cancel_all().await;
    }

    /// Returns a coherent immutable settings snapshot without database I/O.
    pub fn settings(&self) -> Arc<Settings> {
        self.inner
            .settings
            .read()
            .expect("settings lock poisoned")
            .clone()
    }

    /// Stores known setting keys and publishes the resulting snapshot and live configuration.
    ///
    /// Unknown keys are ignored, first duplicate input wins, and invalid stored values
    /// retain their previous cached value, following the settings catalogue's parse rules.
    pub async fn update_settings(
        &self,
        values: Vec<(String, Option<String>)>,
    ) -> Result<(), EngineError> {
        let _settings_guard = self.inner.settings_lock.lock().await;
        let provider_guard = self.inner.provider_lock.lock().await;
        self.inner.store.settings().update(&values).await?;
        let previous = self.settings();
        let mut next = (*previous).clone();
        self.inner.store.settings().load_into(&mut next).await?;
        let rebuilt = if previous.provider == next.provider {
            None
        } else {
            Some(build_provider(
                &next,
                &self.inner.options,
                &self.inner.coordinator,
            )?)
        };
        let next = Arc::new(next);
        if let Some(provider) = rebuilt {
            *self.inner.provider.write().expect("provider lock poisoned") = provider;
        }
        *self.inner.settings.write().expect("settings lock poisoned") = next.clone();
        self.inner
            .internal_config
            .send_replace(internal_config(&next));
        self.inner.disk_config.send_replace(disk_config(&next));
        self.inner.watch_config.send_replace(watch_config(&next));
        drop(provider_guard);
        self.notify_settings(next);
        Ok(())
    }

    /// Updates one persisted setting; unknown keys are ignored.
    pub async fn update_setting(&self, key: &str, value: Option<&str>) -> Result<(), EngineError> {
        self.update_settings(vec![(key.to_owned(), value.map(str::to_owned))])
            .await
    }

    /// Subscribes to disk-space and provider cooldown changes.
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    /// Adjusts provider polling cadence according to whether any authenticated UI is connected.
    pub fn set_ui_connected(&self, connected: bool) {
        self.inner.ui_connected.store(connected, Ordering::Relaxed);
    }

    /// Last relevant disk-space transition; absent before the first pause-related event.
    pub fn disk_space_status(&self) -> Option<DiskSpaceStatus> {
        self.inner
            .disk_status
            .read()
            .expect("disk status lock poisoned")
            .clone()
    }

    /// Current longest provider cooldown, purging expired entries.
    pub fn rate_limit_status(&self) -> RateLimitStatus {
        let next = self.inner.coordinator.max_next_allowed_at();
        RateLimitStatus {
            next_dequeue_time: next,
            seconds_remaining: next.map_or(0.0, |at| {
                at.duration_since(jiff::Timestamp::now())
                    .as_secs_f64()
                    .max(0.0)
            }),
        }
    }

    /// Returns live bytes and speed, extraction percentage, or zeroes for inactive rows.
    pub fn download_stats(&self, download_id: Uuid) -> DownloadStats {
        self.inner.registry.stats(download_id)
    }

    /// Sum of the current speeds of active host transfers in bytes per second.
    pub fn active_download_speed_sum(&self) -> u64 {
        self.inner.registry.speed_sum()
    }

    /// Actual bytes received by host transfers during this engine session.
    pub fn session_downloaded_bytes(&self) -> u64 {
        self.inner.registry.session_bytes()
    }

    /// Subscribes to the live internal downloader settings used by transfer constructors.
    pub fn internal_download_config(&self) -> watch::Receiver<InternalConfig> {
        self.inner.internal_config.subscribe()
    }

    /// Returns the optional injected host-downloader constructor.
    pub fn download_factory(&self) -> Option<Arc<dyn DownloadFactory>> {
        self.inner.options.download_factory.clone()
    }

    /// Returns account details and the latest successfully checked release/advisory state.
    pub async fn profile(&self) -> Result<Profile, EngineError> {
        let provider = self.provider();
        let user = provider
            .user()
            .await
            .inspect_err(|error| self.record_provider_error(error))?;
        let updates = self.inner.updater.state();
        Ok(Profile {
            provider: provider.kind().to_string(),
            user_name: user.username,
            expiration: user.expiration,
            premium: user.premium || user.expiration.is_some(),
            current_version: updates.current,
            latest_version: updates.latest,
            is_insecure: updates.insecure,
            disable_update_notification: self.settings().general.disable_update_notifications,
        })
    }

    pub(crate) fn provider(&self) -> Arc<dyn DebridProvider> {
        self.inner
            .provider
            .read()
            .expect("provider lock poisoned")
            .clone()
    }

    pub(crate) fn record_provider_error(&self, error: &DebridError) {
        if let Some(delay) = error.retry_after() {
            self.inner.coordinator.update_cooldown("General", delay);
            *self
                .inner
                .rate_observed
                .lock()
                .expect("rate status lock poisoned") = self.inner.coordinator.max_next_allowed_at();
            let _ = self
                .inner
                .events
                .send(EngineEvent::RateLimitStatus(self.rate_limit_status()));
        }
    }

    fn notify_settings(&self, settings: Arc<Settings>) {
        if let Some(listener) = &self.inner.options.settings_listener {
            listener(settings);
        }
    }
}

fn build_provider(
    settings: &Settings,
    options: &EngineOptions,
    coordinator: &Arc<RateLimitCoordinator>,
) -> Result<Arc<dyn DebridProvider>, EngineError> {
    let config = ProviderConfig {
        api_key: settings.provider.api_key.clone(),
        api_hostname: settings.provider.api_hostname.clone(),
        timeout: Duration::from_secs(u64::try_from(settings.provider.timeout).unwrap_or(1).max(1)),
        prefer_zipped_downloads: settings.provider.prefer_zipped_downloads,
    };
    let http = ProviderHttp::new(config.timeout, coordinator.clone())?;
    (options.provider_factory)(settings.provider.provider, &config, http)
}

fn internal_config(settings: &Settings) -> InternalConfig {
    let d = &settings.download_client;
    InternalConfig {
        max_speed: u64::try_from(d.max_speed)
            .unwrap_or(0)
            .saturating_mul(1024 * 1024),
        parallel_connections: d.parallel_count,
        chunk_count: d.chunk_count,
        timeout_ms: d.timeout,
        buffer_size: d.buffer_size,
        proxy: d.proxy_server.clone(),
        bind_address: d
            .bind_to_specific_ip
            .then(|| d.bind_ip_address.clone())
            .flatten(),
    }
}

fn disk_config(settings: &Settings) -> DiskSpaceConfig {
    DiskSpaceConfig {
        path: settings.download_client.download_path.clone().into(),
        threshold_gb: settings.download_client.minimum_free_space_gb,
        interval_minutes: settings.download_client.disk_space_check_interval_minutes,
    }
}

fn watch_config(settings: &Settings) -> WatchFolderConfig {
    WatchFolderConfig {
        path: settings.watch.path.clone().unwrap_or_default().into(),
        processed_path: settings.watch.processed_path.as_ref().map(Into::into),
        error_path: settings.watch.error_path.as_ref().map(Into::into),
        interval: Duration::from_secs(u64::try_from(settings.watch.interval).unwrap_or(10)),
    }
}

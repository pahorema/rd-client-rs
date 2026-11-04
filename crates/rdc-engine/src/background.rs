//! Cancellation-aware adapters connecting reusable jobs to the facade.

use crate::{
    Engine, EngineError, EngineEvent, TorrentTemplate,
    disk_space::{self, DiskSpaceAction},
    watch_folder::{self, WatchInput},
};
use std::{convert::Infallible, sync::atomic::Ordering, time::Duration};
use tokio::task::JoinHandle;

pub(crate) fn spawn(engine: &Engine) -> Vec<JoinHandle<()>> {
    let poller = engine.clone();
    let provider = tokio::spawn(async move {
        provider_loop(poller).await;
    });
    let watcher = engine.clone();
    let watch = tokio::spawn(async move {
        let importer = watcher.clone();
        watch_folder::run(
            watcher.inner.watch_config.subscribe(),
            watcher.inner.cancel.clone(),
            move |input| {
                let engine = importer.clone();
                async move {
                    let settings = engine.settings();
                    let template = TorrentTemplate::from_defaults(
                        &settings.watch.default,
                        settings.download_client.client,
                    );
                    match input {
                        WatchInput::Magnet(magnet) => engine.add_magnet(&magnet, template).await?,
                        WatchInput::Torrent(bytes) => {
                            engine.add_torrent_file(&bytes, template).await?
                        }
                    };
                    Ok::<_, EngineError>(())
                }
            },
        )
        .await;
    });
    let disk = engine.clone();
    let disk_job = tokio::spawn(async move {
        let apply = disk.clone();
        disk_space::run(
            disk.inner.disk_config.subscribe(),
            disk.inner.cancel.clone(),
            move |action| {
                let engine = apply.clone();
                async move {
                    match action {
                        DiskSpaceAction::Pause => {
                            engine.inner.low_disk.store(true, Ordering::Relaxed);
                            engine.inner.registry.pause_internal().await;
                        }
                        DiskSpaceAction::Resume => {
                            engine.inner.low_disk.store(false, Ordering::Relaxed);
                            engine.inner.registry.resume_internal().await;
                        }
                        DiskSpaceAction::StatusChanged(status) => {
                            *engine
                                .inner
                                .disk_status
                                .write()
                                .expect("disk status lock poisoned") = Some(status.clone());
                            let _ = engine
                                .inner
                                .events
                                .send(EngineEvent::DiskSpaceStatus(status));
                        }
                    }
                    Ok::<_, Infallible>(())
                }
            },
        )
        .await;
    });
    let update = engine.clone();
    let update_job = tokio::spawn(async move {
        update.inner.updater.run(update.inner.cancel.clone()).await;
    });
    let rate = engine.clone();
    let rate_job = tokio::spawn(async move {
        rate_expiry_loop(rate).await;
    });
    vec![provider, watch, disk_job, update_job, rate_job]
}

async fn provider_loop(engine: Engine) {
    let mut next = tokio::time::Instant::now();
    let mut failures = 0_u32;
    loop {
        if engine.inner.cancel.is_cancelled() {
            return;
        }
        if tokio::time::Instant::now() >= next {
            let settings = engine.settings();
            let connected = engine.inner.ui_connected.load(Ordering::Relaxed);
            let interval = poll_interval(settings.provider.check_interval, connected);
            let result = async {
                let torrents = engine.torrents().await?;
                if needs_provider_poll(&settings, &torrents, connected) {
                    engine.sync_provider().await?;
                    engine.dispatch_queued().await?;
                }
                Ok::<_, EngineError>(())
            };
            let result = tokio::select! {
                () = engine.inner.cancel.cancelled() => return,
                result = result => result,
            };
            let delay = match result {
                Ok(()) => {
                    failures = 0;
                    interval
                }
                Err(error) => {
                    tracing::warn!(%error, "provider poll failed");
                    failures = failures.saturating_add(1);
                    interval
                        .max(Duration::from_secs(1_u64 << failures.min(8)))
                        .max(engine.inner.coordinator.max_remaining_cooldown())
                }
            };
            next = tokio::time::Instant::now() + delay;
        }
        tokio::select! {
            () = engine.inner.cancel.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(1)) => {},
        }
    }
}

fn poll_interval(configured: i32, connected: bool) -> Duration {
    let seconds = u64::try_from(configured).unwrap_or(0);
    Duration::from_secs(if connected {
        seconds.max(5)
    } else {
        seconds.saturating_mul(3).max(30)
    })
}

pub(crate) async fn rate_expiry_loop(engine: Engine) {
    loop {
        let status = engine.rate_limit_status();
        {
            let mut previous = engine
                .inner
                .rate_observed
                .lock()
                .expect("rate status lock poisoned");
            if *previous != status.next_dequeue_time {
                *previous = status.next_dequeue_time;
                let _ = engine
                    .inner
                    .events
                    .send(EngineEvent::RateLimitStatus(status));
            }
        }
        tokio::select! {
            () = engine.inner.cancel.cancelled() => return,
            () = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}

fn needs_provider_poll(
    settings: &rdc_model::settings::Settings,
    torrents: &[rdc_model::Torrent],
    connected: bool,
) -> bool {
    !settings.provider.api_key.trim().is_empty()
        && (settings.provider.auto_import
            || connected
            || torrents.iter().any(|torrent| torrent.completed.is_none()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_errors_do_not_keep_polling_without_import_or_connected_ui() {
        let mut settings = rdc_model::settings::Settings::default();
        settings.provider.api_key = "fake".to_owned();
        let mut torrent = rdc_model::Torrent {
            completed: Some(jiff::Timestamp::now()),
            rd_status: Some(rdc_model::TorrentStatus::Error),
            ..rdc_model::Torrent::default()
        };
        assert!(!needs_provider_poll(&settings, &[torrent.clone()], false));
        assert!(needs_provider_poll(&settings, &[torrent.clone()], true));
        settings.provider.auto_import = true;
        assert!(needs_provider_poll(&settings, &[torrent.clone()], false));
        settings.provider.auto_import = false;
        torrent.completed = None;
        assert!(needs_provider_poll(&settings, &[torrent], false));
        settings.provider.api_key.clear();
        assert!(!needs_provider_poll(&settings, &[], true));
    }

    #[test]
    fn polling_cadence_applies_connected_and_idle_floors() {
        assert_eq!(poll_interval(10, false), Duration::from_secs(30));
        assert_eq!(poll_interval(10, true), Duration::from_secs(10));
        assert_eq!(poll_interval(1, false), Duration::from_secs(30));
        assert_eq!(poll_interval(1, true), Duration::from_secs(5));
        assert_eq!(poll_interval(i32::MAX, false).as_secs(), 6_442_450_941);
    }

    #[tokio::test]
    async fn cooldown_expiration_broadcasts_clear_status_even_after_getter_purges_it() {
        let test = crate::testing::TestEngine::new().await.expect("engine");
        let mut events = test.engine.subscribe();
        test.engine
            .record_provider_error(&rdc_debrid::DebridError::RateLimited {
                message: "slow down".to_owned(),
                retry_after: Duration::from_millis(20),
            });
        assert!(
            matches!(events.recv().await.expect("start event"), EngineEvent::RateLimitStatus(status) if status.next_dequeue_time.is_some())
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(test.engine.rate_limit_status().next_dequeue_time.is_none());
        let task = tokio::spawn(rate_expiry_loop(test.engine.clone()));
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("expiry timeout")
            .expect("expiry event");
        assert_eq!(
            event,
            EngineEvent::RateLimitStatus(crate::RateLimitStatus::default())
        );
        test.engine.shutdown().await;
        task.await.expect("expiry job stopped");
    }
}

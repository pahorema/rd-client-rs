//! Active transfers shared by lifecycle operations, monitoring and host orchestration.

use crate::DownloadStats;
use rdc_download::{Completion, DownloadEngine};
use rdc_model::DownloadClient;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) struct ActiveDownload {
    pub torrent_id: Uuid,
    pub kind: DownloadClient,
    pub engine: Arc<dyn DownloadEngine>,
    pub last_bytes: AtomicU64,
}

#[derive(Debug)]
pub(crate) struct ActiveUnpack {
    pub torrent_id: Uuid,
    pub cancel: CancellationToken,
    pub completion: Completion,
    pub progress: watch::Receiver<u8>,
}

#[derive(Debug, Default)]
pub(crate) struct Registry {
    pub downloads: Mutex<HashMap<Uuid, Arc<ActiveDownload>>>,
    pub unpacks: Mutex<HashMap<Uuid, Arc<ActiveUnpack>>>,
    session_bytes: AtomicU64,
}

impl Registry {
    pub(crate) fn stats(&self, id: Uuid) -> DownloadStats {
        if let Some(active) = self
            .downloads
            .lock()
            .expect("download registry lock poisoned")
            .get(&id)
        {
            return self.download_stats(active);
        }
        self.unpacks
            .lock()
            .expect("unpack registry lock poisoned")
            .get(&id)
            .map_or_else(DownloadStats::default, |active| DownloadStats {
                speed: 0,
                bytes_total: 100,
                bytes_done: u64::from(*active.progress.borrow()).min(100),
            })
    }

    fn download_stats(&self, active: &ActiveDownload) -> DownloadStats {
        let progress = *active.engine.progress().borrow();
        let previous = active
            .last_bytes
            .fetch_max(progress.bytes_done, Ordering::Relaxed);
        self.session_bytes.fetch_add(
            progress.bytes_done.saturating_sub(previous),
            Ordering::Relaxed,
        );
        DownloadStats {
            speed: progress.speed,
            bytes_total: progress.bytes_total,
            bytes_done: progress.bytes_done,
        }
    }

    pub(crate) fn speed_sum(&self) -> u64 {
        self.downloads
            .lock()
            .expect("download registry lock poisoned")
            .values()
            .map(|active| self.download_stats(active).speed)
            .fold(0, u64::saturating_add)
    }

    pub(crate) fn session_bytes(&self) -> u64 {
        // Capture outstanding deltas even when no stats endpoint was queried.
        self.speed_sum();
        self.session_bytes.load(Ordering::Relaxed)
    }

    pub(crate) async fn cancel(&self, id: Uuid) {
        let download = self
            .downloads
            .lock()
            .expect("download registry lock poisoned")
            .get(&id)
            .cloned();
        if let Some(active) = download {
            active.engine.cancel().await;
            self.download_stats(&active);
            let mut downloads = self
                .downloads
                .lock()
                .expect("download registry lock poisoned");
            if downloads
                .get(&id)
                .is_some_and(|entry| Arc::ptr_eq(entry, &active))
            {
                downloads.remove(&id);
            }
        }
        let unpack = self
            .unpacks
            .lock()
            .expect("unpack registry lock poisoned")
            .get(&id)
            .cloned();
        if let Some(active) = unpack {
            active.cancel.cancel();
            let mut completion = active.completion.clone();
            let _ = completion.wait().await;
            let mut unpacks = self.unpacks.lock().expect("unpack registry lock poisoned");
            if unpacks
                .get(&id)
                .is_some_and(|entry| Arc::ptr_eq(entry, &active))
            {
                unpacks.remove(&id);
            }
        }
    }

    pub(crate) async fn cancel_torrent(&self, torrent_id: Uuid) {
        let mut ids: Vec<_> = self
            .downloads
            .lock()
            .expect("download registry lock poisoned")
            .iter()
            .filter_map(|(id, active)| (active.torrent_id == torrent_id).then_some(*id))
            .collect();
        ids.extend(
            self.unpacks
                .lock()
                .expect("unpack registry lock poisoned")
                .iter()
                .filter_map(|(id, active)| (active.torrent_id == torrent_id).then_some(*id)),
        );
        for id in ids {
            self.cancel(id).await;
        }
    }

    pub(crate) async fn cancel_all(&self) {
        let mut ids: Vec<_> = self
            .downloads
            .lock()
            .expect("download registry lock poisoned")
            .keys()
            .copied()
            .collect();
        ids.extend(
            self.unpacks
                .lock()
                .expect("unpack registry lock poisoned")
                .keys()
                .copied(),
        );
        for id in ids {
            self.cancel(id).await;
        }
    }

    pub(crate) async fn pause_torrent(&self, torrent_id: Uuid) {
        self.set_paused(Some(torrent_id), false, true).await;
    }
    pub(crate) async fn resume_torrent(&self, torrent_id: Uuid) {
        self.set_paused(Some(torrent_id), false, false).await;
    }
    pub(crate) async fn pause_internal(&self) {
        self.set_paused(None, true, true).await;
    }
    pub(crate) async fn resume_internal(&self) {
        self.set_paused(None, true, false).await;
    }

    async fn set_paused(&self, torrent_id: Option<Uuid>, internal_only: bool, paused: bool) {
        let active: Vec<_> = self
            .downloads
            .lock()
            .expect("download registry lock poisoned")
            .values()
            .filter(|active| {
                torrent_id.is_none_or(|id| id == active.torrent_id)
                    && (!internal_only || active.kind == DownloadClient::Internal)
            })
            .cloned()
            .collect();
        for download in active {
            if paused {
                download.engine.pause().await;
            } else {
                download.engine.resume().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeleteOptions, TorrentTemplate, test_download::TestDownload, testing::TestEngine};
    use rdc_download::Progress;

    fn register(
        registry: &Registry,
        id: Uuid,
        torrent_id: Uuid,
        kind: DownloadClient,
        engine: Arc<TestDownload>,
    ) {
        registry.downloads.lock().expect("registry").insert(
            id,
            Arc::new(ActiveDownload {
                torrent_id,
                kind,
                engine,
                last_bytes: AtomicU64::new(0),
            }),
        );
    }

    #[tokio::test]
    async fn stats_sum_speeds_and_count_session_byte_deltas_once() {
        let registry = Registry::default();
        let first = Arc::new(TestDownload::new(false));
        let second = Arc::new(TestDownload::new(false));
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        register(
            &registry,
            first_id,
            Uuid::new_v4(),
            DownloadClient::Internal,
            first.clone(),
        );
        register(
            &registry,
            second_id,
            Uuid::new_v4(),
            DownloadClient::Aria2c,
            second.clone(),
        );
        first.progress.send_replace(Progress {
            bytes_done: 20,
            bytes_total: 100,
            speed: 7,
        });
        second.progress.send_replace(Progress {
            bytes_done: 30,
            bytes_total: 100,
            speed: 11,
        });
        assert_eq!(registry.speed_sum(), 18);
        assert_eq!(registry.session_bytes(), 50);
        assert_eq!(registry.stats(first_id).bytes_done, 20);
        assert_eq!(registry.session_bytes(), 50);
        first.progress.send_replace(Progress {
            bytes_done: 35,
            bytes_total: 100,
            speed: 0,
        });
        assert_eq!(registry.session_bytes(), 65);
        registry.cancel_all().await;
        assert_eq!(
            registry.session_bytes(),
            65,
            "cancellation does not lose received bytes"
        );
        assert_eq!(registry.stats(first_id), DownloadStats::default());
    }

    #[tokio::test]
    async fn low_disk_pause_only_targets_internal_transfers_and_resume_is_live() {
        let registry = Registry::default();
        let internal = Arc::new(TestDownload::new(false));
        let remote = Arc::new(TestDownload::new(false));
        let torrent_id = Uuid::new_v4();
        register(
            &registry,
            Uuid::new_v4(),
            torrent_id,
            DownloadClient::Internal,
            internal.clone(),
        );
        register(
            &registry,
            Uuid::new_v4(),
            torrent_id,
            DownloadClient::Aria2c,
            remote.clone(),
        );
        registry.pause_internal().await;
        assert_eq!(internal.paused.load(Ordering::Relaxed), 1);
        assert_eq!(remote.paused.load(Ordering::Relaxed), 0);
        registry.resume_internal().await;
        assert_eq!(internal.resumed.load(Ordering::Relaxed), 1);
        registry.pause_torrent(torrent_id).await;
        assert_eq!(remote.paused.load(Ordering::Relaxed), 1);
        registry.resume_torrent(torrent_id).await;
        assert_eq!(remote.resumed.load(Ordering::Relaxed), 1);
        registry.cancel_all().await;
    }

    #[tokio::test]
    async fn delete_waits_for_active_transfer_to_stop_before_removing_rows() {
        let test = TestEngine::new().await.expect("engine");
        let torrent = test
            .engine
            .add_magnet(
                "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=test",
                TorrentTemplate::default(),
            )
            .await
            .expect("torrent");
        let transfer = Arc::new(TestDownload::new(true));
        register(
            &test.engine.inner.registry,
            Uuid::new_v4(),
            torrent.torrent_id,
            DownloadClient::Internal,
            transfer.clone(),
        );
        let engine = test.engine.clone();
        let deletion = tokio::spawn(async move {
            engine
                .delete(
                    torrent.torrent_id,
                    DeleteOptions {
                        data: true,
                        ..DeleteOptions::default()
                    },
                )
                .await
        });
        transfer.cancel_started.notified().await;
        assert!(!deletion.is_finished(), "delete must await cancellation");
        assert!(
            test.store
                .torrents()
                .get(torrent.torrent_id)
                .await
                .expect("row")
                .is_some()
        );
        transfer.cancel_release.notify_one();
        deletion.await.expect("delete task").expect("delete");
        assert!(
            test.store
                .torrents()
                .get(torrent.torrent_id)
                .await
                .expect("row")
                .is_none()
        );
        test.engine.shutdown().await;
    }

    #[tokio::test]
    async fn unpack_stats_are_real_percent_and_cancellation_waits_for_completion() {
        let registry = Arc::new(Registry::default());
        let (sender, completion) = rdc_download::completion_channel();
        let (_progress, receiver) = watch::channel(68_u8);
        let cancel = CancellationToken::new();
        let id = Uuid::new_v4();
        registry.unpacks.lock().expect("unpacks").insert(
            id,
            Arc::new(ActiveUnpack {
                torrent_id: Uuid::new_v4(),
                cancel: cancel.clone(),
                completion,
                progress: receiver,
            }),
        );
        assert_eq!(
            registry.stats(id),
            DownloadStats {
                speed: 0,
                bytes_total: 100,
                bytes_done: 68
            }
        );
        let active = registry.clone();
        let task = tokio::spawn(async move {
            active.cancel(id).await;
        });
        cancel.cancelled().await;
        assert!(!task.is_finished());
        sender.complete(Ok(()));
        task.await.expect("stopped");
        assert_eq!(registry.stats(id), DownloadStats::default());
    }
}

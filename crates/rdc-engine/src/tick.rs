//! Ordered torrent lifecycle phases, serialized across manual and background ticks.

use crate::{Engine, EngineError, registry::ActiveDownload};
use jiff::{SignedDuration, Timestamp};
use rdc_download::Outcome;
use rdc_model::{
    Download, DownloadClient, Torrent, TorrentHostDownloadAction, TorrentStatus, paths,
};
use rdc_store::{AddResult, RetryOnError};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

impl Engine {
    /// Advances torrent lifecycle work once, without overlapping another manual or background tick.
    ///
    /// Provider and host limits pause only new starts; selection, extraction, error handling
    /// and completion continue. Shutdown interrupts in-flight work and makes later calls inert.
    ///
    /// # Errors
    /// Returns persistence errors or failures outside an individual torrent's lifecycle phases.
    pub async fn tick_once(&self) -> Result<(), EngineError> {
        let _guard = self.inner.tick_lock.lock().await;
        tokio::select! {
            biased;
            () = self.inner.cancel.cancelled() => Ok(()),
            result = self.tick_inner() => result,
        }
    }

    async fn tick_inner(&self) -> Result<(), EngineError> {
        let settings = self.settings();
        if settings.provider.api_key.trim().is_empty()
            || settings.download_client.download_path.trim().is_empty()
        {
            return Ok(());
        }
        // Remote download engines own their asynchronous status polling.
        self.collect_completed_downloads().await?;
        self.collect_completed_unpacks().await?;
        let torrents = self.torrents().await?;
        self.remove_stale_work(&torrents).await;
        self.process_retention_and_retries(&torrents).await?;
        self.dispatch_queued().await?;
        let torrents = self.torrents().await?;
        self.process_finished_actions(&torrents).await?;
        let download_limit = usize::try_from(settings.general.download_limit.max(1)).unwrap_or(1);
        let unpack_limit = usize::try_from(settings.general.unpack_limit.max(0)).unwrap_or(0);
        let mut starts_blocked = false;
        for torrent in torrents
            .iter()
            .filter(|torrent| torrent.completed.is_none())
        {
            if let Err(error) = self
                .advance_torrent(torrent, download_limit, unpack_limit, &mut starts_blocked)
                .await
            {
                let message = format!("Runner error: {error}");
                self.inner
                    .store
                    .torrents()
                    .update_complete(
                        torrent.torrent_id,
                        Some(&message),
                        Some(Timestamp::now()),
                        RetryOnError::Schedule,
                    )
                    .await?;
                tracing::error!(torrent_id = %torrent.torrent_id, %error, "torrent processing failed");
            }
        }
        Ok(())
    }

    async fn advance_torrent(
        &self,
        torrent: &Torrent,
        download_limit: usize,
        unpack_limit: usize,
        starts_blocked: &mut bool,
    ) -> Result<(), EngineError> {
        if !*starts_blocked {
            *starts_blocked = self.start_queued_downloads(torrent, download_limit).await?;
        }
        self.start_unpacks(torrent, unpack_limit).await?;
        if torrent.rd_status == Some(TorrentStatus::Error) {
            let message = format!(
                "Debrid error: {}.",
                torrent.rd_status_raw.as_deref().unwrap_or_default()
            );
            self.inner
                .store
                .torrents()
                .update_complete(
                    torrent.torrent_id,
                    Some(&message),
                    Some(Timestamp::now()),
                    RetryOnError::Schedule,
                )
                .await?;
            return Ok(());
        }
        if matches!(
            torrent.rd_status,
            Some(TorrentStatus::WaitingForFileSelection | TorrentStatus::Finished)
        ) && torrent.files_selected.is_none()
            && torrent.downloads.is_empty()
        {
            self.select_torrent_files(torrent.torrent_id).await?;
        }
        let Some(current) = self.inner.store.torrents().get(torrent.torrent_id).await? else {
            return Ok(());
        };
        if current.completed.is_some() {
            return Ok(());
        }
        if current.rd_status == Some(TorrentStatus::Finished)
            && current.downloads.is_empty()
            && current.files_selected.is_some()
            && current.host_download_action == TorrentHostDownloadAction::DownloadAll
        {
            self.create_torrent_downloads(&current).await?;
        }
        // Selection and download creation are visible to completion in the same tick.
        if let Some(current) = self.inner.store.torrents().get(torrent.torrent_id).await? {
            self.complete_if_ready(&current).await?;
        }
        Ok(())
    }

    async fn select_torrent_files(&self, id: Uuid) -> Result<(), EngineError> {
        let Some(torrent) = self.torrent(id).await? else {
            return Ok(());
        };
        let selected = self
            .provider()
            .select_files(&torrent)
            .await
            .inspect_err(|error| self.record_provider_error(error))?;
        match selected {
            None => tracing::warn!(torrent_id = %id, "file list not yet available from provider"),
            Some(0) => self.mark_all_files_excluded(&torrent).await?,
            Some(_) => {
                self.inner
                    .store
                    .torrents()
                    .update_files_selected(id, Timestamp::now())
                    .await?;
            }
        }
        Ok(())
    }

    async fn create_torrent_downloads(&self, torrent: &Torrent) -> Result<(), EngineError> {
        let Some(infos) = self
            .provider()
            .download_infos(torrent)
            .await
            .inspect_err(|error| self.record_provider_error(error))?
        else {
            return Ok(());
        };
        if infos.is_empty() {
            return self.mark_all_files_excluded(torrent).await;
        }
        for info in &infos {
            match self
                .inner
                .store
                .downloads()
                .try_add(torrent.torrent_id, info)
                .await?
            {
                AddResult::Added | AddResult::AlreadyExists | AddResult::InvalidInput => {}
                AddResult::TorrentMissing => break,
            }
        }
        Ok(())
    }

    async fn mark_all_files_excluded(&self, torrent: &Torrent) -> Result<(), EngineError> {
        self.inner
            .store
            .torrents()
            .update_retry(torrent.torrent_id, None, torrent.torrent_retry_attempts)
            .await?;
        self.inner
            .store
            .torrents()
            .update_complete(
                torrent.torrent_id,
                Some("All files excluded"),
                Some(Timestamp::now()),
                RetryOnError::Skip,
            )
            .await?;
        Ok(())
    }

    async fn start_queued_downloads(
        &self,
        torrent: &Torrent,
        limit: usize,
    ) -> Result<bool, EngineError> {
        let now = Timestamp::now();
        let mut queued: Vec<_> = torrent
            .downloads
            .iter()
            .filter(|download| {
                download.completed.is_none()
                    && download.error.is_none()
                    && download.download_started.is_none()
                    && download.download_queued.is_some_and(|at| at <= now)
            })
            .collect();
        queued.sort_by_key(|download| download.download_queued);
        for original in queued {
            let blocked = {
                let active = self
                    .inner
                    .registry
                    .downloads
                    .lock()
                    .expect("download registry lock poisoned");
                active.contains_key(&original.download_id)
                    || (torrent.download_client != DownloadClient::Symlink && active.len() >= limit)
            };
            if blocked
                || (torrent.download_client == DownloadClient::Internal
                    && self.inner.low_disk.load(Ordering::Relaxed))
            {
                return Ok(true);
            }
            let mut download = original.clone();
            if let Err(error) = self.resolve_download(torrent, &mut download).await {
                self.unrestrict_failed(torrent, &download, &error).await?;
                return Ok(true);
            }
            let engine = match self.prepare_download(torrent, &download).await {
                Ok(engine) => engine,
                Err(error) => {
                    self.finish_download(download.download_id, Err(error.to_string()))
                        .await?;
                    continue;
                }
            };
            let active = Arc::new(ActiveDownload {
                torrent_id: torrent.torrent_id,
                kind: torrent.download_client,
                engine,
                last_bytes: AtomicU64::new(0),
            });
            {
                let _guard = self.inner.provider_lock.lock().await;
                let Some(current) = self
                    .inner
                    .store
                    .downloads()
                    .get(download.download_id)
                    .await?
                else {
                    continue;
                };
                if current.torrent.completed.is_some() || current.download != download {
                    continue;
                }
                self.inner
                    .store
                    .downloads()
                    .update_download_started(download.download_id, Some(Timestamp::now()))
                    .await?;
                self.inner
                    .registry
                    .downloads
                    .lock()
                    .expect("download registry lock poisoned")
                    .insert(download.download_id, active.clone());
            }
            match active.engine.start().await {
                Ok(id) => {
                    if let Err(error) = self
                        .inner
                        .store
                        .downloads()
                        .update_remote_id(download.download_id, &id)
                        .await
                    {
                        self.inner.registry.cancel(download.download_id).await;
                        return Err(error.into());
                    }
                    if torrent.download_client == DownloadClient::Internal
                        && self.inner.low_disk.load(Ordering::Relaxed)
                    {
                        active.engine.pause().await;
                    }
                }
                Err(error) => {
                    let registered = self
                        .inner
                        .registry
                        .downloads
                        .lock()
                        .expect("download registry lock poisoned")
                        .get(&download.download_id)
                        .is_some_and(|current| Arc::ptr_eq(current, &active));
                    if registered {
                        self.finish_download(download.download_id, Err(error.to_string()))
                            .await?;
                    }
                }
            }
        }
        Ok(false)
    }

    async fn resolve_download(
        &self,
        torrent: &Torrent,
        download: &mut Download,
    ) -> Result<(), EngineError> {
        if download.link.is_some() {
            return Ok(());
        }
        let provider = self.provider();
        if download.retry_count > 0
            && let Some(infos) = provider
                .download_infos(torrent)
                .await
                .inspect_err(|error| self.record_provider_error(error))?
            && let Some(info) = paths::match_download_info(&infos, download)
            && info.restricted_link != download.path
            // A link that already belongs to a sibling download is not taken over.
            && self
                .inner
                .store
                .downloads()
                .update_path(download.download_id, &info.restricted_link)
                .await?
        {
            download.path.clone_from(&info.restricted_link);
        }
        let link = provider
            .unrestrict(torrent, &download.path)
            .await
            .inspect_err(|error| self.record_provider_error(error))?;
        self.inner
            .store
            .downloads()
            .update_link(download.download_id, &link)
            .await?;
        download.link = Some(link);
        if download.file_name.is_none() {
            let name = provider
                .file_name(download)
                .await
                .inspect_err(|error| self.record_provider_error(error))?;
            self.inner
                .store
                .downloads()
                .update_file_name(download.download_id, &name)
                .await?;
            download.file_name = Some(name);
        }
        Ok(())
    }

    async fn unrestrict_failed(
        &self,
        torrent: &Torrent,
        download: &Download,
        error: &EngineError,
    ) -> Result<(), EngineError> {
        if let EngineError::Provider(error) = error
            && let Some(delay) = error.retry_after()
        {
            let retry_at = self
                .inner
                .coordinator
                .max_next_allowed_at()
                .unwrap_or_else(|| add_duration(Timestamp::now(), delay));
            self.inner
                .store
                .downloads()
                .reset(download.download_id, Some(retry_at))
                .await?;
            self.inner
                .store
                .downloads()
                .update_retry_count(download.download_id, download.retry_count)
                .await?;
            return Ok(());
        }
        let _guard = self.inner.provider_lock.lock().await;
        if download.retry_count < torrent.download_retry_attempts {
            let count = download.retry_count.saturating_add(1);
            let seconds = match count {
                i32::MIN..=1 => 15,
                2 => 30,
                3 => 60,
                _ => 120,
            };
            self.inner
                .store
                .downloads()
                .reset(
                    download.download_id,
                    Some(add_duration(Timestamp::now(), Duration::from_secs(seconds))),
                )
                .await?;
            self.inner
                .store
                .downloads()
                .update_retry_count(download.download_id, count)
                .await?;
        } else {
            self.inner
                .store
                .downloads()
                .update_error(download.download_id, Some(&error.to_string()))
                .await?;
            self.inner
                .store
                .downloads()
                .update_completed(download.download_id, Some(Timestamp::now()))
                .await?;
        }
        Ok(())
    }

    async fn collect_completed_downloads(&self) -> Result<(), EngineError> {
        let finished: Vec<_> = self
            .inner
            .registry
            .downloads
            .lock()
            .expect("download registry lock poisoned")
            .iter()
            .filter_map(|(id, active)| {
                active
                    .engine
                    .completion()
                    .outcome()
                    .map(|outcome| (*id, outcome))
            })
            .collect();
        for (id, outcome) in finished {
            self.finish_download(id, outcome).await?;
        }
        Ok(())
    }

    async fn finish_download(&self, id: Uuid, outcome: Outcome) -> Result<(), EngineError> {
        let _guard = self.inner.provider_lock.lock().await;
        self.inner.registry.stats(id);
        if let Some(record) = self.inner.store.downloads().get(id).await? {
            match outcome {
                Ok(()) => {
                    self.inner
                        .store
                        .downloads()
                        .update_download_finished(id, Some(Timestamp::now()))
                        .await?;
                    self.inner
                        .store
                        .downloads()
                        .update_unpacking_queued(id, Some(Timestamp::now()))
                        .await?;
                }
                Err(error)
                    if record.download.retry_count < record.torrent.download_retry_attempts =>
                {
                    self.inner.store.downloads().reset(id, None).await?;
                    self.inner
                        .store
                        .downloads()
                        .update_retry_count(id, record.download.retry_count.saturating_add(1))
                        .await?;
                    tracing::debug!(download_id = %id, %error, "retrying host transfer");
                }
                Err(error) => {
                    self.inner
                        .store
                        .downloads()
                        .update_error(id, Some(&error))
                        .await?;
                    self.inner
                        .store
                        .downloads()
                        .update_completed(id, Some(Timestamp::now()))
                        .await?;
                }
            }
        }
        self.inner
            .registry
            .downloads
            .lock()
            .expect("download registry lock poisoned")
            .remove(&id);
        Ok(())
    }

    async fn remove_stale_work(&self, torrents: &[Torrent]) {
        let ids: HashSet<_> = torrents
            .iter()
            .flat_map(|torrent| {
                torrent
                    .downloads
                    .iter()
                    .map(|download| download.download_id)
            })
            .collect();
        let stale_download = self
            .inner
            .registry
            .downloads
            .lock()
            .expect("download registry lock poisoned")
            .keys()
            .find(|id| !ids.contains(id))
            .copied();
        if let Some(id) = stale_download {
            self.inner.registry.cancel(id).await;
        }
        let stale_unpack = self
            .inner
            .registry
            .unpacks
            .lock()
            .expect("unpack registry lock poisoned")
            .keys()
            .find(|id| !ids.contains(id))
            .copied();
        if let Some(id) = stale_unpack {
            self.inner.registry.cancel(id).await;
        }
    }

    async fn process_retention_and_retries(&self, torrents: &[Torrent]) -> Result<(), EngineError> {
        for torrent in torrents.iter().filter(|torrent| torrent.retry.is_some()) {
            if torrent.retry_count > torrent.torrent_retry_attempts {
                self.inner
                    .store
                    .torrents()
                    .update_retry(torrent.torrent_id, None, torrent.retry_count)
                    .await?;
            } else if let Err(error) = self
                .retry_torrent(torrent.torrent_id, torrent.retry_count)
                .await
            {
                self.inner
                    .store
                    .torrents()
                    .update_retry(torrent.torrent_id, None, torrent.retry_count)
                    .await?;
                self.inner
                    .store
                    .torrents()
                    .update_error(torrent.torrent_id, &error.to_string())
                    .await?;
            }
        }
        for torrent in torrents {
            if torrent.error.is_some()
                && torrent.delete_on_error > 0
                && torrent
                    .completed
                    .is_some_and(|at| minutes_elapsed(at, torrent.delete_on_error))
            {
                self.delete(
                    torrent.torrent_id,
                    crate::DeleteOptions {
                        data: true,
                        provider: true,
                        local_files: true,
                    },
                )
                .await?;
            }
        }
        for torrent in torrents {
            if torrent.downloads.is_empty()
                && torrent.completed.is_none()
                && torrent.lifetime > 0
                && minutes_elapsed(torrent.added, torrent.lifetime)
            {
                self.inner
                    .store
                    .torrents()
                    .update_retry(torrent.torrent_id, None, torrent.torrent_retry_attempts)
                    .await?;
                self.inner
                    .store
                    .torrents()
                    .update_complete(
                        torrent.torrent_id,
                        Some(&format!(
                            "Torrent lifetime of {} minutes reached",
                            torrent.lifetime
                        )),
                        Some(Timestamp::now()),
                        RetryOnError::Skip,
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

fn add_duration(at: Timestamp, duration: Duration) -> Timestamp {
    at.checked_add(SignedDuration::try_from(duration).unwrap_or(SignedDuration::MAX))
        .unwrap_or(Timestamp::MAX)
}
pub(crate) fn minutes_elapsed(at: Timestamp, minutes: i32) -> bool {
    Timestamp::now()
        >= at
            .checked_add(SignedDuration::from_secs(i64::from(minutes) * 60))
            .unwrap_or(Timestamp::MAX)
}

#[cfg(test)]
#[path = "tick_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tick_lifecycle_tests.rs"]
mod lifecycle_tests;

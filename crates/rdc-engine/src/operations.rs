//! Torrent queries, validated additions, queue dispatch and explicit lifecycle operations.

use std::{
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use data_encoding::BASE64;
use jiff::Timestamp;
use rdc_debrid::AvailableFile;
use rdc_model::{Torrent, TorrentStatus, paths};
use rdc_store::{NewTorrent, RetryOnError};
use rdc_torrent::{MagnetLink, TorrentFile};
use tokio::{fs, time};
use uuid::Uuid;

use crate::{
    DeleteOptions, Engine, EngineError, TorrentTemplate, TorrentUpdate, WaitOutcome,
    copy_added::{self, AddedPayload},
};

#[cfg(test)]
#[path = "operation_tests.rs"]
mod operation_tests;

/// Queue waits are capped even when a caller requests a longer interval.
const MAX_DEQUEUE_WAIT: Duration = Duration::from_secs(90);
/// Polling only reads local rows and does not force provider requests.
const DEQUEUE_POLL: Duration = Duration::from_millis(100);

impl Engine {
    /// Lists torrents with downloads in priority/added order, without contacting the provider.
    ///
    /// # Errors
    /// Returns persistence errors.
    pub async fn torrents(&self) -> Result<Vec<Torrent>, EngineError> {
        Ok(self.inner.store.torrents().list().await?)
    }

    /// Loads a torrent including its payload and refreshes its provider mirror when present.
    ///
    /// # Errors
    /// Returns persistence errors and provider rate limits; other refresh failures are logged.
    pub async fn torrent(&self, torrent_id: Uuid) -> Result<Option<Torrent>, EngineError> {
        let _guard = self.inner.provider_lock.lock().await;
        let Some(mut torrent) = self.inner.store.torrents().get(torrent_id).await? else {
            return Ok(None);
        };
        self.refresh_torrent(&mut torrent, None).await?;
        Ok(Some(torrent))
    }

    /// Finds a case-insensitive info hash and refreshes the matching torrent's provider mirror.
    ///
    /// # Errors
    /// Returns persistence errors and provider rate limits.
    pub async fn torrent_by_hash(&self, hash: &str) -> Result<Option<Torrent>, EngineError> {
        let _guard = self.inner.provider_lock.lock().await;
        let Some(mut torrent) = self.inner.store.torrents().find_by_hash(hash).await? else {
            return Ok(None);
        };
        self.refresh_torrent(&mut torrent, None).await?;
        Ok(Some(torrent))
    }

    /// Validates and enqueues a magnet, enriching trackers and attempting immediate provider dispatch.
    ///
    /// Duplicate hashes return the existing torrent unchanged. Tracker-list and copy failures
    /// are logged without rejecting a valid addition. Provider failures are stored on the row.
    ///
    /// # Errors
    /// Returns invalid-magnet, banned-tracker and persistence errors.
    pub async fn add_magnet(
        &self,
        magnet: &str,
        template: TorrentTemplate,
    ) -> Result<Torrent, EngineError> {
        self.add_magnet_with_retry_count(magnet, template, None)
            .await
    }

    async fn add_magnet_with_retry_count(
        &self,
        magnet: &str,
        template: TorrentTemplate,
        retry_count: Option<i32>,
    ) -> Result<Torrent, EngineError> {
        let parsed = MagnetLink::parse(magnet)
            .map_err(|error| EngineError::Message(format!("{error}, trying to parse {magnet}")))?;
        let settings = self.settings();
        if let Some(error) = rdc_torrent::banned_magnet_error(
            settings
                .general
                .banned_trackers
                .as_deref()
                .unwrap_or_default(),
            &parsed,
        ) {
            return Err(EngineError::Message(error));
        }
        let trackers = self.enrichment_trackers().await;
        let enriched = rdc_torrent::enrich_magnet(magnet, &trackers);
        let mut template = template.into_torrent();
        template.rd_name = parsed.display_name.clone();
        template.rd_status = Some(TorrentStatus::Queued);
        self.add_queued(
            &parsed.info_hash_hex(),
            &enriched,
            false,
            &template,
            retry_count,
        )
        .await
    }

    /// Validates and enqueues torrent metadata, storing and copying the enriched payload.
    ///
    /// Hashes and banned-tracker checks use the submitted metadata. Duplicate hashes keep
    /// their existing configuration and payload. Tracker fetch failures do not prevent addition.
    ///
    /// # Errors
    /// Returns invalid-metadata, banned-tracker and persistence errors.
    pub async fn add_torrent_file(
        &self,
        bytes: &[u8],
        template: TorrentTemplate,
    ) -> Result<Torrent, EngineError> {
        self.add_file_with_retry_count(bytes, template, None).await
    }

    async fn add_file_with_retry_count(
        &self,
        bytes: &[u8],
        template: TorrentTemplate,
        retry_count: Option<i32>,
    ) -> Result<Torrent, EngineError> {
        let parsed =
            TorrentFile::parse(bytes).map_err(|error| torrent_parse_error(&error, bytes))?;
        let settings = self.settings();
        if let Some(error) = rdc_torrent::banned_torrent_error(
            settings
                .general
                .banned_trackers
                .as_deref()
                .unwrap_or_default(),
            &parsed,
        ) {
            return Err(EngineError::Message(error));
        }
        let trackers = self.enrichment_trackers().await;
        let enriched = rdc_torrent::enrich_torrent(bytes, &trackers).map_err(message)?;
        let payload = BASE64.encode(&enriched);
        let mut template = template.into_torrent();
        template.rd_name = Some(parsed.name.clone());
        template.rd_status = Some(TorrentStatus::Queued);
        self.add_queued(
            &parsed.info_hash_hex(),
            &payload,
            true,
            &template,
            retry_count,
        )
        .await
    }

    async fn enrichment_trackers(&self) -> Arc<[String]> {
        let settings = self.settings();
        match self
            .inner
            .trackers
            .fetch(
                settings.general.tracker_enrichment_list.as_deref(),
                settings.general.tracker_enrichment_cache_expiration,
                &self.inner.cancel,
            )
            .await
        {
            Ok(trackers) => trackers,
            Err(error) => {
                tracing::warn!(%error, "Adding torrent without tracker enrichment");
                Arc::from([])
            }
        }
    }

    async fn add_queued(
        &self,
        hash: &str,
        payload: &str,
        is_file: bool,
        template: &Torrent,
        retry_count: Option<i32>,
    ) -> Result<Torrent, EngineError> {
        let (torrent, inserted) = {
            // Share the provider lock with sync/import so the duplicate check and insertion
            // cannot race another addition or a provider-side import.
            let _guard = self.inner.provider_lock.lock().await;
            let (mut torrent, inserted) =
                if let Some(existing) = self.inner.store.torrents().find_by_hash(hash).await? {
                    (existing, false)
                } else {
                    let torrent = self
                        .inner
                        .store
                        .torrents()
                        .add(&NewTorrent {
                            rd_id: None,
                            hash,
                            payload: Some(payload),
                            is_file,
                            download_client: template.download_client,
                            template,
                        })
                        .await?;
                    (torrent, true)
                };
            if let Some(count) = retry_count {
                self.inner
                    .store
                    .torrents()
                    .update_retry(torrent.torrent_id, None, count)
                    .await?;
                torrent.retry_count = count;
                torrent.retry = None;
            }
            (torrent, inserted)
        };
        self.copy_added_torrent(&torrent).await;
        if !inserted {
            return Ok(torrent);
        }
        self.dispatch_queued().await?;
        Ok(self
            .inner
            .store
            .torrents()
            .get(torrent.torrent_id)
            .await?
            .unwrap_or(torrent))
    }

    async fn copy_added_torrent(&self, torrent: &Torrent) {
        let settings = self.settings();
        let Some(directory) = settings
            .general
            .copy_added_torrents
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return;
        };
        let stored;
        let payload = if let Some(payload) = torrent.payload.as_deref() {
            payload
        } else {
            stored = match self
                .inner
                .store
                .torrents()
                .payload(torrent.torrent_id)
                .await
            {
                Ok(payload) => payload,
                Err(error) => {
                    tracing::warn!(%error, "Unable to read torrent copy payload");
                    return;
                }
            };
            let Some(payload) = stored.as_deref() else {
                return;
            };
            payload
        };
        if torrent.is_file {
            match BASE64.decode(payload.as_bytes()) {
                Ok(bytes) => {
                    copy_added::copy_added(
                        Some(Path::new(directory)),
                        torrent.rd_name.as_deref(),
                        AddedPayload::Torrent(&bytes),
                    )
                    .await;
                }
                Err(error) => tracing::warn!(%error, "Unable to decode torrent copy payload"),
            }
        } else {
            copy_added::copy_added(
                Some(Path::new(directory)),
                torrent.rd_name.as_deref(),
                AddedPayload::Magnet(payload),
            )
            .await;
        }
    }

    /// Dispatches eligible rows in priority order while sharing the provider sync lock.
    pub(crate) async fn dispatch_queued(&self) -> Result<(), EngineError> {
        let _guard = self.inner.provider_lock.lock().await;
        let settings = self.settings();
        if settings.provider.api_key.trim().is_empty()
            || self.inner.coordinator.max_next_allowed_at().is_some()
        {
            return Ok(());
        }
        let torrents = self.inner.store.torrents().list().await?;
        let mut active = torrents
            .iter()
            .filter(|torrent| {
                torrent.completed.is_none()
                    && torrent.rd_id.is_some()
                    && !matches!(
                        torrent.rd_status,
                        Some(TorrentStatus::Finished | TorrentStatus::Error)
                    )
            })
            .count();
        let limit = usize::try_from(settings.provider.max_parallel_downloads).unwrap_or_default();
        for torrent in torrents.iter().filter(|torrent| is_queued(torrent)) {
            if (limit > 0 && active >= limit)
                || self.inner.coordinator.max_next_allowed_at().is_some()
            {
                break;
            }
            let result = self.dispatch_torrent(torrent.torrent_id).await;
            match result {
                Ok(()) => active += 1,
                Err(EngineError::Provider(error)) if error.retry_after().is_some() => {
                    break;
                }
                Err(error) => {
                    let error = format!("Could not add to provider: {error}");
                    self.inner
                        .store
                        .torrents()
                        .update_complete(
                            torrent.torrent_id,
                            Some(&error),
                            Some(Timestamp::now()),
                            RetryOnError::Schedule,
                        )
                        .await?;
                    tracing::warn!(torrent_id = %torrent.torrent_id, %error, "Provider addition failed");
                }
            }
        }
        Ok(())
    }

    async fn dispatch_torrent(&self, torrent_id: Uuid) -> Result<(), EngineError> {
        let Some(mut torrent) = self.inner.store.torrents().get(torrent_id).await? else {
            return Ok(());
        };
        if !is_queued(&torrent) {
            return Ok(());
        }
        let payload = torrent
            .payload
            .as_deref()
            .filter(|payload| !payload.trim().is_empty())
            .ok_or_else(|| {
                EngineError::Message("Torrent has no torrent file or magnet link".into())
            })?;
        let provider = self.provider();
        let result = if torrent.is_file {
            let bytes = BASE64.decode(payload.as_bytes()).map_err(message)?;
            provider.add_torrent_file(&bytes).await
        } else {
            provider.add_magnet(payload).await
        };
        let id = result.map_err(|error| {
            self.record_provider_error(&error);
            EngineError::Provider(error)
        })?;
        if id.trim().is_empty() {
            return Err(EngineError::Message(
                "No torrent ID received from provider".into(),
            ));
        }
        self.inner
            .store
            .torrents()
            .update_rd_id(torrent_id, &id)
            .await?;
        torrent.rd_id = Some(id);
        torrent.client_kind = Some(provider.kind());
        // Persist identity before refreshing so a refresh failure never loses a successful add.
        self.inner.store.torrents().update_rd_data(&torrent).await?;
        self.refresh_torrent(&mut torrent, None).await
    }

    /// Cancels active work before independently deleting rows, provider data and local files.
    ///
    /// Keeping rows marks the torrent completed with `Torrent deleted`. Provider deletion
    /// failures are logged; rate limits still update the shared cooldown. Local deletion never
    /// follows a symlink outside the configured download directory.
    ///
    /// # Errors
    /// Returns persistence errors or local cleanup errors after three attempts.
    pub async fn delete(
        &self,
        torrent_id: Uuid,
        options: DeleteOptions,
    ) -> Result<(), EngineError> {
        let _guard = self.inner.provider_lock.lock().await;
        self.delete_locked(torrent_id, options).await
    }

    pub(crate) async fn delete_locked(
        &self,
        torrent_id: Uuid,
        options: DeleteOptions,
    ) -> Result<(), EngineError> {
        let Some(torrent) = self.inner.store.torrents().get(torrent_id).await? else {
            return Ok(());
        };
        self.inner
            .store
            .torrents()
            .update_complete(
                torrent_id,
                Some("Torrent deleted"),
                Some(Timestamp::now()),
                RetryOnError::Skip,
            )
            .await?;
        self.inner.registry.cancel_torrent(torrent_id).await;
        if options.data {
            self.inner.store.torrents().delete(torrent_id).await?;
        }
        if options.provider
            && torrent.rd_id.is_some()
            && let Err(error) = self.provider().delete(&torrent).await
        {
            self.record_provider_error(&error);
            tracing::warn!(%torrent_id, %error, "Unable to delete torrent at provider");
        }
        if options.local_files {
            let settings = self.settings();
            if let Some(path) = torrent_directory(&torrent) {
                remove_local(Path::new(&settings.download_client.download_path), &path).await?;
            }
        }
        Ok(())
    }

    /// Requests a manual whole-torrent retry with a fresh retry allowance.
    ///
    /// # Errors
    /// Returns persistence, missing-payload, invalid-payload or local cleanup errors.
    pub async fn retry(&self, torrent_id: Uuid) -> Result<(), EngineError> {
        self.inner
            .store
            .torrents()
            .update_retry(torrent_id, Some(Timestamp::now()), 0)
            .await?;
        self.retry_torrent(torrent_id, 0).await
    }

    pub(crate) async fn retry_torrent(
        &self,
        torrent_id: Uuid,
        retry_count: i32,
    ) -> Result<(), EngineError> {
        let _reset = self.inner.reset_lock.lock().await;
        let Some(torrent) = self.inner.store.torrents().get(torrent_id).await? else {
            return Ok(());
        };
        if torrent.retry.is_none() {
            return Ok(());
        }
        self.inner
            .store
            .torrents()
            .update_complete(
                torrent_id,
                Some("Retrying torrent"),
                Some(Timestamp::now()),
                RetryOnError::Skip,
            )
            .await?;
        self.inner
            .store
            .torrents()
            .update_retry(torrent_id, None, 0)
            .await?;
        for download in &torrent.downloads {
            self.inner
                .store
                .downloads()
                .update_error(download.download_id, None)
                .await?;
            self.inner
                .store
                .downloads()
                .update_completed(download.download_id, Some(Timestamp::now()))
                .await?;
        }
        self.delete(
            torrent_id,
            DeleteOptions {
                data: true,
                provider: true,
                local_files: true,
            },
        )
        .await?;
        let payload = torrent
            .payload
            .as_deref()
            .filter(|payload| !payload.trim().is_empty())
            .ok_or_else(|| {
                EngineError::Message(
                    "Cannot re-add this torrent: stored magnet or file not found".into(),
                )
            })?;
        let template = TorrentTemplate::from(&torrent);
        if torrent.is_file {
            let bytes = BASE64.decode(payload.as_bytes()).map_err(message)?;
            self.add_file_with_retry_count(&bytes, template, Some(retry_count))
                .await?;
        } else {
            self.add_magnet_with_retry_count(payload, template, Some(retry_count))
                .await?;
        }
        Ok(())
    }

    /// Cancels a download and extraction, deletes its local file, and reopens the torrent.
    ///
    /// The restricted link, remote transfer id and filename are preserved; retry counters,
    /// progress timestamps, unrestricted link and errors are reset.
    ///
    /// # Errors
    /// Returns persistence or local cleanup errors. Missing downloads are ignored.
    pub async fn retry_download(&self, download_id: Uuid) -> Result<(), EngineError> {
        let _reset = self.inner.reset_lock.lock().await;
        let _provider = self.inner.provider_lock.lock().await;
        let Some(record) = self.inner.store.downloads().get(download_id).await? else {
            return Ok(());
        };
        self.inner.registry.cancel(download_id).await;
        let mut torrent = record.torrent;
        torrent.rd_name = torrent.rd_name.as_deref().and_then(safe_component);
        let mut download = record.download;
        download.file_name = paths::file_name(&download).and_then(|name| safe_component(&name));
        if download.file_name.is_some()
            && let Some(relative) = paths::relative_download_path(&torrent, &download)
        {
            let settings = self.settings();
            let mut path = category_directory(&torrent);
            path.push(relative);
            remove_local(Path::new(&settings.download_client.download_path), &path).await?;
        }
        self.inner
            .store
            .downloads()
            .reset(download_id, None)
            .await?;
        self.inner
            .store
            .torrents()
            .update_complete(torrent.torrent_id, None, None, RetryOnError::Skip)
            .await?;
        Ok(())
    }

    /// Replaces exactly the eight user-editable torrent options, leaving lifecycle state untouched.
    ///
    /// # Errors
    /// Returns persistence errors. Missing torrents are ignored.
    pub async fn update(&self, torrent_id: Uuid, update: TorrentUpdate) -> Result<(), EngineError> {
        let torrent = Torrent {
            torrent_id,
            download_client: update.download_client,
            host_download_action: update.host_download_action,
            category: update.category,
            priority: update.priority,
            download_retry_attempts: update.download_retry_attempts,
            torrent_retry_attempts: update.torrent_retry_attempts,
            delete_on_error: update.delete_on_error,
            lifetime: update.lifetime,
            ..Torrent::default()
        };
        self.inner.store.torrents().update(&torrent).await?;
        Ok(())
    }

    /// Changes the category for a hash without moving existing files or changing active transfers.
    ///
    /// # Errors
    /// Returns persistence errors. Missing hashes are ignored.
    pub async fn set_category(
        &self,
        hash: &str,
        category: Option<String>,
    ) -> Result<(), EngineError> {
        if let Some(torrent) = self.inner.store.torrents().find_by_hash(hash).await? {
            self.inner
                .store
                .torrents()
                .update_category(torrent.torrent_id, category.as_deref())
                .await?;
        }
        Ok(())
    }

    /// Changes queue ordering without preempting active work; lower priorities run first.
    ///
    /// # Errors
    /// Returns persistence errors. Missing hashes are ignored.
    pub async fn set_priority(&self, hash: &str, priority: Option<i32>) -> Result<(), EngineError> {
        if let Some(torrent) = self.inner.store.torrents().find_by_hash(hash).await? {
            self.inner
                .store
                .torrents()
                .update_priority(torrent.torrent_id, priority)
                .await?;
        }
        Ok(())
    }

    /// Sets local selection flags by zero-based file-list index, ignoring out-of-range indices.
    ///
    /// This does not contact the provider. Empty input, absent torrents and empty file lists
    /// return `false`; provider file identifiers are not interpreted as list indices.
    ///
    /// # Errors
    /// Returns persistence or file-list serialization errors.
    pub async fn update_file_selection(
        &self,
        hash: &str,
        file_ids: &[usize],
        selected: bool,
    ) -> Result<bool, EngineError> {
        if file_ids.is_empty() {
            return Ok(false);
        }
        let _guard = self.inner.provider_lock.lock().await;
        let Some(mut torrent) = self.inner.store.torrents().find_by_hash(hash).await? else {
            return Ok(false);
        };
        let mut files = torrent.files();
        if files.is_empty() {
            return Ok(false);
        }
        for &index in file_ids {
            if let Some(file) = files.get_mut(index) {
                file.selected = selected;
            }
        }
        torrent.rd_files = Some(serde_json::to_string(&files).map_err(message)?);
        self.inner.store.torrents().update_rd_data(&torrent).await?;
        Ok(true)
    }

    /// Pauses currently active transfers only; the pause is not persisted.
    ///
    /// # Errors
    /// Returns persistence errors; missing hashes are ignored and transfer errors are logged.
    pub async fn pause(&self, hash: &str) -> Result<(), EngineError> {
        if let Some(torrent) = self.inner.store.torrents().find_by_hash(hash).await? {
            self.inner.registry.pause_torrent(torrent.torrent_id).await;
        }
        Ok(())
    }

    /// Resumes currently active transfers without changing queued or completed rows.
    ///
    /// # Errors
    /// Returns persistence errors; missing hashes are ignored and transfer errors are logged.
    pub async fn resume(&self, hash: &str) -> Result<(), EngineError> {
        if let Some(torrent) = self.inner.store.torrents().find_by_hash(hash).await? {
            self.inner.registry.resume_torrent(torrent.torrent_id).await;
        }
        Ok(())
    }

    /// Lists cached provider files for a lowercase-normalized info hash without modifying rows.
    ///
    /// # Errors
    /// Returns provider errors and records any requested cooldown.
    pub async fn available_files(&self, hash: &str) -> Result<Vec<AvailableFile>, EngineError> {
        self.provider()
            .available_files(&hash.to_lowercase())
            .await
            .map_err(|error| {
                self.record_provider_error(&error);
                error.into()
            })
    }

    /// Parses a magnet and queries cached provider files without adding a torrent.
    ///
    /// # Errors
    /// Returns magnet parsing or provider errors and records requested provider cooldowns.
    pub async fn available_files_for_magnet(
        &self,
        magnet: &str,
    ) -> Result<Vec<AvailableFile>, EngineError> {
        let parsed = MagnetLink::parse(magnet)
            .map_err(|error| EngineError::Message(format!("{error}, trying to parse {magnet}")))?;
        self.available_files(&parsed.info_hash_hex()).await
    }

    /// Parses torrent metadata and queries cached provider files without adding a torrent.
    ///
    /// # Errors
    /// Returns metadata parsing or provider errors and records requested provider cooldowns.
    pub async fn available_files_for_torrent(
        &self,
        bytes: &[u8],
    ) -> Result<Vec<AvailableFile>, EngineError> {
        let parsed =
            TorrentFile::parse(bytes).map_err(|error| torrent_parse_error(&error, bytes))?;
        self.available_files(&parsed.info_hash_hex()).await
    }

    /// Waits for provider dispatch, deletion, completion or shutdown, for at most ninety seconds.
    ///
    /// Timeouts are ordinary outcomes, not errors. No provider requests are made by this wait.
    ///
    /// # Errors
    /// Returns persistence errors while inspecting the local torrent row.
    pub async fn wait_until_dequeued(
        &self,
        torrent_id: Uuid,
        timeout: Duration,
    ) -> Result<WaitOutcome, EngineError> {
        let wait = async {
            loop {
                let Some(torrent) = self.inner.store.torrents().get(torrent_id).await? else {
                    return Ok(WaitOutcome::Missing);
                };
                if torrent.rd_id.is_some() {
                    return Ok(WaitOutcome::Dequeued);
                }
                if torrent.completed.is_some() || torrent.error.is_some() {
                    return Ok(WaitOutcome::Completed);
                }
                time::sleep(DEQUEUE_POLL).await;
            }
        };
        tokio::select! {
            biased;
            () = self.inner.cancel.cancelled() => Ok(WaitOutcome::Shutdown),
            result = time::timeout(timeout.min(MAX_DEQUEUE_WAIT), wait) => result.unwrap_or(Ok(WaitOutcome::TimedOut)),
        }
    }
}

/// Describes invalid torrent metadata by its size; the bytes themselves can be megabytes.
fn torrent_parse_error(error: &impl std::fmt::Display, bytes: &[u8]) -> EngineError {
    EngineError::Message(format!(
        "{error}, trying to parse a {}-byte torrent file",
        bytes.len()
    ))
}

fn is_queued(torrent: &Torrent) -> bool {
    torrent.completed.is_none()
        && torrent.error.is_none()
        && torrent.rd_id.is_none()
        && torrent.rd_added.is_none()
        && torrent.rd_status == Some(TorrentStatus::Queued)
}

fn message(error: impl std::fmt::Display) -> EngineError {
    EngineError::Message(error.to_string())
}

fn safe_component(value: &str) -> Option<String> {
    let value = paths::remove_invalid_file_name_chars(value);
    (!value.trim().is_empty() && value != ".").then_some(value)
}

fn category_directory(torrent: &Torrent) -> PathBuf {
    torrent
        .category
        .as_deref()
        .and_then(safe_component)
        .map_or_else(PathBuf::new, PathBuf::from)
}

fn torrent_directory(torrent: &Torrent) -> Option<PathBuf> {
    let name = safe_component(torrent.rd_name.as_deref()?)?;
    Some(category_directory(torrent).join(name))
}

/// Never follow parent symlinks during cleanup, and unlink a final symlink itself.
async fn remove_local(base: &Path, relative: &Path) -> io::Result<()> {
    if base.as_os_str().is_empty()
        || relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe local cleanup path",
        ));
    }
    let root = match fs::canonicalize(base).await {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let destination = root.join(relative);
    let mut parent = root.clone();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            break;
        }
        parent.push(component);
        match fs::symlink_metadata(&parent).await {
            Ok(metadata) if metadata.is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "local cleanup parent is a symlink",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    for attempt in 0..3 {
        let metadata = match fs::symlink_metadata(&destination).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let result = if metadata.is_dir() && !metadata.is_symlink() {
            fs::remove_dir_all(&destination).await
        } else {
            fs::remove_file(&destination).await
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) if attempt == 2 => return Err(error),
            Err(_) => time::sleep(Duration::from_secs(1)).await,
        }
    }
    Ok(())
}

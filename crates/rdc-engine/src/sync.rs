//! Provider state refresh, account imports and provider-id reconciliation.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use rdc_debrid::{RemoteTorrent, is_spool_id};
use rdc_model::{Provider, Torrent, TorrentStatus};
use rdc_store::NewTorrent;

use crate::{DeleteOptions, Engine, EngineError, TorrentTemplate};

impl Engine {
    /// Refreshes the provider mirror without acquiring the provider mutation lock.
    ///
    /// Callers already holding that lock can refresh after adding or deleting a
    /// torrent. Ordinary provider failures leave the stored mirror untouched;
    /// rate limits reach the coordinator and the caller.
    pub(crate) async fn refresh_torrent(
        &self,
        torrent: &mut Torrent,
        remote: Option<&RemoteTorrent>,
    ) -> Result<(), EngineError> {
        if torrent.rd_id.is_none() {
            return Ok(());
        }
        let before = ProviderState::from(&*torrent);
        if let Err(error) = self.provider().refresh(torrent, remote).await {
            if error.retry_after().is_some() {
                self.record_provider_error(&error);
                return Err(error.into());
            }
            tracing::warn!(torrent_id = %torrent.torrent_id, %error, "provider refresh failed");
            return Ok(());
        }
        if !before.matches(torrent) {
            self.inner.store.torrents().update_rd_data(torrent).await?;
        }
        Ok(())
    }

    /// Synchronizes provider mirrors, imports account torrents and removes missing torrents.
    ///
    /// Imports use the current provider defaults and do not contain a retry
    /// payload. Queued and provider-finished torrents are never auto-deleted.
    ///
    /// # Errors
    ///
    /// Returns listing, persistence and deletion errors. Refresh errors are
    /// logged and ignored unless they request a provider cooldown.
    pub async fn sync_provider(&self) -> Result<(), EngineError> {
        let _guard = self.inner.provider_lock.lock().await;
        let settings = self.settings();
        let provider = self.provider();
        let mut torrents = self.inner.store.torrents().list().await?;
        let remote_torrents = provider
            .list_torrents()
            .await
            .inspect_err(|error| self.record_provider_error(error))?;
        let mut by_id: HashMap<String, usize> = torrents
            .iter()
            .enumerate()
            .filter_map(|(index, torrent)| {
                torrent
                    .rd_id
                    .as_ref()
                    .filter(|id| !id.trim().is_empty())
                    .map(|id| (id.clone(), index))
            })
            .collect();

        for remote in &remote_torrents {
            let index = if let Some(&index) = by_id.get(&remote.id) {
                index
            } else if let Some(index) = migration_candidate(&torrents, remote, provider.kind()) {
                let torrent = &mut torrents[index];
                self.inner
                    .store
                    .torrents()
                    .update_rd_id(torrent.torrent_id, &remote.id)
                    .await?;
                if let Some(old_id) = torrent.rd_id.replace(remote.id.clone()) {
                    by_id.remove(&old_id);
                }
                torrent.client_kind = Some(provider.kind());
                self.inner.store.torrents().update_rd_data(torrent).await?;
                by_id.insert(remote.id.clone(), index);
                tracing::info!(
                    torrent_id = %torrent.torrent_id,
                    provider_id = %remote.id,
                    provider = ?provider.kind(),
                    "reconciled provider torrent id"
                );
                index
            } else if settings.provider.auto_import {
                let template = TorrentTemplate::from_defaults(
                    &settings.provider.default,
                    settings.download_client.client,
                )
                .into_torrent();
                let torrent = self
                    .inner
                    .store
                    .torrents()
                    .add(&NewTorrent {
                        rd_id: Some(&remote.id),
                        hash: &remote.hash,
                        payload: None,
                        is_file: false,
                        download_client: settings.download_client.client,
                        template: &template,
                    })
                    .await?;
                let index = torrents.len();
                torrents.push(torrent);
                by_id.insert(remote.id.clone(), index);
                index
            } else {
                continue;
            };
            self.refresh_torrent(&mut torrents[index], Some(remote))
                .await?;
        }

        // The shared deletion implementation must not reacquire our lock.
        if settings.provider.auto_delete {
            let remote_ids: HashSet<&str> = remote_torrents
                .iter()
                .map(|remote| remote.id.as_str())
                .collect();
            for torrent in torrents {
                if !matches!(
                    torrent.rd_status,
                    Some(TorrentStatus::Queued | TorrentStatus::Finished)
                ) && torrent
                    .rd_id
                    .as_deref()
                    .is_none_or(|id| !remote_ids.contains(id))
                {
                    self.delete_locked(
                        torrent.torrent_id,
                        DeleteOptions {
                            data: true,
                            provider: false,
                            local_files: true,
                        },
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }
}

fn migration_candidate(
    torrents: &[Torrent],
    remote: &RemoteTorrent,
    kind: Provider,
) -> Option<usize> {
    match kind {
        Provider::TorBox if !remote.id.trim().is_empty() && !remote.hash.trim().is_empty() => {
            torrents.iter().position(|torrent| {
                torrent
                    .client_kind
                    .is_none_or(|kind| kind == Provider::TorBox)
                    && !torrent.hash.trim().is_empty()
                    && torrent.hash.eq_ignore_ascii_case(&remote.hash)
                    && torrent.rd_id.as_deref().is_some_and(|id| {
                        !id.trim().is_empty() && id.eq_ignore_ascii_case(&torrent.hash)
                    })
            })
        }
        Provider::Highway
            if remote.id.trim().parse::<i64>().is_ok() && !remote.filename.trim().is_empty() =>
        {
            torrents.iter().position(|torrent| {
                torrent
                    .client_kind
                    .is_none_or(|kind| kind == Provider::Highway)
                    && torrent.rd_id.as_deref().is_some_and(is_spool_id)
                    && torrent.rd_name.as_deref().is_some_and(|name| {
                        let mut other = remote.filename.chars();
                        name.chars().all(|left| {
                            other.next().is_some_and(|right| {
                                left == right || left.to_uppercase().eq(right.to_uppercase())
                            })
                        }) && other.next().is_none()
                    })
            })
        }
        _ => None,
    }
}

// Snapshot only persisted provider columns, not payloads or download collections.
struct ProviderState {
    kind: Option<Provider>,
    name: Option<String>,
    size: Option<i64>,
    host: Option<String>,
    split: Option<i64>,
    progress: Option<i64>,
    status: Option<TorrentStatus>,
    status_raw: Option<String>,
    added: Option<Timestamp>,
    ended: Option<Timestamp>,
    speed: Option<i64>,
    seeders: Option<i64>,
    files: Option<String>,
}

impl From<&Torrent> for ProviderState {
    fn from(torrent: &Torrent) -> Self {
        Self {
            kind: torrent.client_kind,
            name: torrent.rd_name.clone(),
            size: torrent.rd_size,
            host: torrent.rd_host.clone(),
            split: torrent.rd_split,
            progress: torrent.rd_progress,
            status: torrent.rd_status,
            status_raw: torrent.rd_status_raw.clone(),
            added: torrent.rd_added,
            ended: torrent.rd_ended,
            speed: torrent.rd_speed,
            seeders: torrent.rd_seeders,
            files: torrent.rd_files.clone(),
        }
    }
}

impl ProviderState {
    fn matches(&self, torrent: &Torrent) -> bool {
        self.kind == torrent.client_kind
            && self.name == torrent.rd_name
            && self.size == torrent.rd_size
            && self.host == torrent.rd_host
            && self.split == torrent.rd_split
            && self.progress == torrent.rd_progress
            && self.status == torrent.rd_status
            && self.status_raw == torrent.rd_status_raw
            && self.added == torrent.rd_added
            && self.ended == torrent.rd_ended
            && self.speed == torrent.rd_speed
            && self.seeders == torrent.rd_seeders
            && self.files == torrent.rd_files
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rdc_debrid::DebridError;
    use rdc_model::{
        DownloadClient, RemoteFile, TorrentDownloadAction, TorrentFinishedAction,
        TorrentHostDownloadAction, settings::Settings,
    };

    use super::*;
    use crate::testing::{FakeDebridProvider, ProviderCall, ProviderOperation, TestEngine};

    fn remote(id: &str) -> RemoteTorrent {
        RemoteTorrent {
            id: id.to_owned(),
            filename: "Example".to_owned(),
            hash: "abcdef0123456789abcdef0123456789abcdef01".to_owned(),
            bytes: 8 * 1024 * 1024 * 1024,
            original_bytes: 9 * 1024 * 1024 * 1024,
            host: Some("provider-host".to_owned()),
            split: 17,
            progress: 53,
            status: Some("downloading".to_owned()),
            added: Some(Timestamp::from_second(1_700_000_000).expect("valid timestamp")),
            ended: Some(Timestamp::from_second(1_700_000_100).expect("valid timestamp")),
            speed: Some(1234),
            seeders: Some(12),
            files: Some(vec![RemoteFile {
                id: 7,
                path: "Example/video.mkv".to_owned(),
                bytes: 8 * 1024 * 1024 * 1024,
                selected: true,
                download_link: Some("https://example.test/video.mkv".to_owned()),
            }]),
            ..RemoteTorrent::default()
        }
    }

    async fn insert(
        fixture: &TestEngine,
        id: Option<&str>,
        hash: &str,
        template: &Torrent,
    ) -> Torrent {
        fixture
            .store
            .torrents()
            .add(&NewTorrent {
                rd_id: id,
                hash,
                payload: None,
                is_file: false,
                download_client: template.download_client,
                template,
            })
            .await
            .expect("insert torrent")
    }

    async fn stored(fixture: &TestEngine, torrent: &Torrent) -> Torrent {
        fixture
            .store
            .torrents()
            .get(torrent.torrent_id)
            .await
            .expect("query torrent")
            .expect("torrent exists")
    }

    #[tokio::test]
    async fn refresh_persists_every_provider_column_and_preserves_local_options() {
        let fixture = TestEngine::new().await.expect("test engine");
        let remote = remote("42");
        let mut torrent = insert(
            &fixture,
            Some(&remote.id),
            &remote.hash,
            &Torrent {
                category: Some("series".to_owned()),
                priority: Some(3),
                ..Torrent::default()
            },
        )
        .await;
        fixture
            .engine
            .refresh_torrent(&mut torrent, Some(&remote))
            .await
            .expect("refresh torrent");

        let persisted = stored(&fixture, &torrent).await;
        assert!(ProviderState::from(&torrent).matches(&persisted));
        assert_eq!(persisted.rd_name.as_deref(), Some("Example"));
        assert_eq!(persisted.rd_size, Some(remote.bytes));
        assert_eq!(persisted.rd_host, remote.host);
        assert_eq!(persisted.rd_split, Some(remote.split));
        assert_eq!(persisted.rd_progress, Some(remote.progress));
        assert_eq!(persisted.rd_added, remote.added);
        assert_eq!(persisted.rd_ended, remote.ended);
        assert_eq!(persisted.rd_speed, remote.speed);
        assert_eq!(persisted.rd_seeders, remote.seeders);
        assert_eq!(persisted.files(), remote.files.expect("remote files"));
        assert_eq!(persisted.category.as_deref(), Some("series"));
        assert_eq!(persisted.priority, Some(3));
    }

    #[tokio::test]
    async fn refresh_persists_a_provider_kind_only_change() {
        let fixture = TestEngine::new().await.expect("test engine");
        let remote = remote("42");
        let mut torrent = insert(&fixture, Some("42"), &remote.hash, &Torrent::default()).await;
        fixture
            .engine
            .refresh_torrent(&mut torrent, Some(&remote))
            .await
            .expect("initial refresh");
        let kind = torrent.client_kind.expect("fake assigns provider kind");
        torrent.client_kind = None;
        fixture
            .store
            .torrents()
            .update_rd_data(&torrent)
            .await
            .expect("clear kind");

        fixture
            .engine
            .refresh_torrent(&mut torrent, Some(&remote))
            .await
            .expect("refresh identical mirror");
        assert_eq!(stored(&fixture, &torrent).await.client_kind, Some(kind));
    }

    #[tokio::test]
    async fn ordinary_refresh_errors_are_swallowed_without_changing_persistence() {
        let fixture = TestEngine::new().await.expect("test engine");
        let mut torrent = insert(&fixture, Some("42"), "hash", &Torrent::default()).await;
        let before = torrent.clone();
        fixture.fake.queue_failure(
            ProviderOperation::Refresh,
            DebridError::Provider("temporary provider failure".to_owned()),
        );
        fixture
            .engine
            .refresh_torrent(&mut torrent, None)
            .await
            .expect("ordinary error swallowed");
        assert_eq!(stored(&fixture, &torrent).await, before);
    }

    #[tokio::test]
    async fn refresh_rate_limits_propagate_and_leave_the_mirror_untouched() {
        let fixture = TestEngine::new().await.expect("test engine");
        let mut events = fixture.engine.subscribe();
        let mut torrent = insert(&fixture, Some("42"), "hash", &Torrent::default()).await;
        let before = torrent.clone();
        fixture.fake.queue_failure(
            ProviderOperation::Refresh,
            DebridError::RateLimited {
                message: "slow down".to_owned(),
                retry_after: Duration::from_secs(30),
            },
        );
        let error = fixture
            .engine
            .refresh_torrent(&mut torrent, None)
            .await
            .expect_err("rate limit");
        assert!(matches!(
            error,
            EngineError::Provider(DebridError::RateLimited { .. })
        ));
        assert_eq!(stored(&fixture, &torrent).await, before);
        let status = fixture.engine.rate_limit_status();
        assert!(status.next_dequeue_time.is_some());
        assert!(status.seconds_remaining > 0.0 && status.seconds_remaining <= 30.0);
        assert!(matches!(
            events.try_recv().expect("cooldown event"),
            crate::EngineEvent::RateLimitStatus(event) if event.next_dequeue_time == status.next_dequeue_time
        ));
    }

    #[tokio::test]
    async fn refresh_without_a_listing_persists_deleted_provider_state() {
        let fixture = TestEngine::new().await.expect("test engine");
        let mut torrent = insert(&fixture, Some("missing"), "hash", &Torrent::default()).await;
        fixture
            .engine
            .refresh_torrent(&mut torrent, None)
            .await
            .expect("refresh deleted torrent");
        assert_eq!(
            stored(&fixture, &torrent).await.rd_status_raw.as_deref(),
            Some("deleted")
        );
        assert!(fixture.fake.calls().iter().any(|call| matches!(
            call,
            ProviderCall::Refresh { torrent_id, remote: None } if *torrent_id == torrent.torrent_id
        )));
    }

    #[tokio::test]
    async fn sync_migrates_torbox_hash_ids_without_importing_duplicates() {
        let fake = FakeDebridProvider::new(Provider::TorBox);
        let remote = remote("42");
        fake.set_torrents(vec![remote.clone()]);
        let mut settings = Settings::default();
        settings.provider.auto_import = true;
        let fixture = TestEngine::with_provider(settings, fake)
            .await
            .expect("test engine");
        let torrent = insert(
            &fixture,
            Some(&remote.hash.to_uppercase()),
            &remote.hash,
            &Torrent::default(),
        )
        .await;
        fixture.engine.sync_provider().await.expect("sync provider");
        let persisted = stored(&fixture, &torrent).await;
        assert_eq!(persisted.rd_id.as_deref(), Some("42"));
        assert_eq!(persisted.client_kind, Some(Provider::TorBox));
        assert_eq!(
            fixture.store.torrents().list().await.expect("list").len(),
            1
        );
    }

    #[tokio::test]
    async fn sync_promotes_highway_spool_ids_by_case_insensitive_name() {
        let fake = FakeDebridProvider::new(Provider::Highway);
        let mut remote = remote("42");
        remote.filename = "Éxample".to_owned();
        fake.set_torrents(vec![remote]);
        let fixture = TestEngine::with_provider(Settings::default(), fake)
            .await
            .expect("test engine");
        let torrent = insert(
            &fixture,
            Some("0123456789abcdef0123456789abcdef"),
            "hash",
            &Torrent {
                rd_name: Some("éXAMPLE".to_owned()),
                ..Torrent::default()
            },
        )
        .await;
        fixture.engine.sync_provider().await.expect("sync provider");
        let persisted = stored(&fixture, &torrent).await;
        assert_eq!(persisted.rd_id.as_deref(), Some("42"));
        assert_eq!(persisted.client_kind, Some(Provider::Highway));
        assert_eq!(
            fixture.store.torrents().list().await.expect("list").len(),
            1
        );
    }

    #[test]
    fn id_migrations_reject_other_provider_ownership_and_invalid_spool_ids() {
        let remote = remote("42");
        let mut torrent = Torrent {
            hash: remote.hash.clone(),
            rd_id: Some(remote.hash.clone()),
            client_kind: Some(Provider::RealDebrid),
            ..Torrent::default()
        };
        assert_eq!(
            migration_candidate(&[torrent.clone()], &remote, Provider::TorBox),
            None
        );
        torrent.client_kind = None;
        torrent.rd_name = Some(remote.filename.clone());
        torrent.rd_id = Some("g123456789abcdef0123456789abcdef".to_owned());
        assert_eq!(
            migration_candidate(&[torrent.clone()], &remote, Provider::Highway),
            None
        );
        torrent.rd_id = Some("0123456789abcdef0123456789abcdef".to_owned());
        let mut non_numeric = remote;
        non_numeric.id = "not-a-transfer-id".to_owned();
        assert_eq!(
            migration_candidate(&[torrent], &non_numeric, Provider::Highway),
            None
        );
    }

    #[tokio::test]
    async fn auto_import_copies_provider_defaults_without_creating_a_retry_payload() {
        let fake = FakeDebridProvider::new(Provider::TorBox);
        fake.set_torrents(vec![remote("42")]);
        let mut settings = Settings::default();
        settings.download_client.client = DownloadClient::Aria2c;
        settings.provider.auto_import = true;
        let defaults = &mut settings.provider.default;
        defaults.category = Some("series".to_owned());
        defaults.host_download_action = TorrentHostDownloadAction::DownloadNone;
        defaults.finished_action = TorrentFinishedAction::RemoveProvider;
        defaults.finished_action_delay = 7;
        defaults.only_download_available_files = false;
        defaults.min_file_size = 123;
        defaults.include_regex = Some("include".to_owned());
        defaults.exclude_regex = Some("exclude".to_owned());
        defaults.torrent_retry_attempts = 5;
        defaults.download_retry_attempts = 6;
        defaults.delete_on_error = 8;
        defaults.torrent_lifetime = 9;
        defaults.priority = 10;
        let fixture = TestEngine::with_provider(settings, fake)
            .await
            .expect("test engine");
        fixture.engine.sync_provider().await.expect("sync import");
        fixture
            .engine
            .sync_provider()
            .await
            .expect("sync does not duplicate import");
        let torrents = fixture
            .store
            .torrents()
            .list()
            .await
            .expect("list torrents");
        assert_eq!(torrents.len(), 1);
        let torrent = stored(&fixture, &torrents[0]).await;
        assert_eq!(torrent.rd_id.as_deref(), Some("42"));
        assert_eq!(torrent.download_client, DownloadClient::Aria2c);
        assert_eq!(torrent.category.as_deref(), Some("series"));
        assert_eq!(
            torrent.host_download_action,
            TorrentHostDownloadAction::DownloadNone
        );
        assert_eq!(
            torrent.finished_action,
            TorrentFinishedAction::RemoveProvider
        );
        assert_eq!(torrent.finished_action_delay, 7);
        assert_eq!(torrent.download_action, TorrentDownloadAction::DownloadAll);
        assert_eq!(torrent.download_min_size, 123);
        assert_eq!(torrent.include_regex.as_deref(), Some("include"));
        assert_eq!(torrent.exclude_regex.as_deref(), Some("exclude"));
        assert_eq!(torrent.torrent_retry_attempts, 5);
        assert_eq!(torrent.download_retry_attempts, 6);
        assert_eq!(torrent.delete_on_error, 8);
        assert_eq!(torrent.lifetime, 9);
        assert_eq!(torrent.priority, Some(10));
        assert!(!torrent.is_file);
        assert!(torrent.payload.is_none());
    }

    #[tokio::test]
    async fn disabled_auto_import_leaves_unknown_provider_torrents_untracked() {
        let fixture = TestEngine::new().await.expect("test engine");
        fixture.fake.set_torrents(vec![remote("42")]);
        fixture.engine.sync_provider().await.expect("sync provider");
        assert_eq!(
            fixture.store.torrents().list().await.expect("list"),
            Vec::new()
        );
    }

    #[tokio::test]
    async fn auto_delete_removes_only_missing_nonqueued_unfinished_torrents() {
        let mut settings = Settings::default();
        settings.provider.auto_delete = true;
        let fixture = TestEngine::with_settings(settings)
            .await
            .expect("test engine");
        let mut retained = Vec::new();
        for (id, status) in [
            ("queued", Some(TorrentStatus::Queued)),
            ("finished", Some(TorrentStatus::Finished)),
            ("processing", Some(TorrentStatus::Processing)),
            ("unknown", None),
        ] {
            let torrent = insert(
                &fixture,
                Some(id),
                id,
                &Torrent {
                    rd_status: status,
                    ..Torrent::default()
                },
            )
            .await;
            if matches!(
                status,
                Some(TorrentStatus::Queued | TorrentStatus::Finished)
            ) {
                retained.push(torrent.torrent_id);
            }
        }
        insert(&fixture, None, "no-provider-id", &Torrent::default()).await;
        tokio::time::timeout(Duration::from_secs(5), fixture.engine.sync_provider())
            .await
            .expect("sync must not deadlock during deletion")
            .expect("sync auto deletion");
        let torrents = fixture.store.torrents().list().await.expect("list");
        assert_eq!(torrents.len(), retained.len());
        assert!(
            torrents
                .iter()
                .all(|torrent| retained.contains(&torrent.torrent_id))
        );
        assert!(
            !fixture
                .fake
                .calls()
                .iter()
                .any(|call| matches!(call, ProviderCall::Delete { .. }))
        );
    }

    #[tokio::test]
    async fn listing_failures_release_the_provider_lock_for_the_next_sync() {
        let fixture = TestEngine::new().await.expect("test engine");
        fixture.fake.queue_failure(
            ProviderOperation::ListTorrents,
            DebridError::Provider("listing unavailable".to_owned()),
        );
        assert!(fixture.engine.sync_provider().await.is_err());
        tokio::time::timeout(Duration::from_secs(5), fixture.engine.sync_provider())
            .await
            .expect("provider lock released after failure")
            .expect("next sync succeeds");
    }
    #[tokio::test]
    async fn sync_waits_for_the_provider_mutation_lock() {
        let fixture = TestEngine::new().await.expect("test engine");
        let guard = fixture.engine.inner.provider_lock.lock().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), fixture.engine.sync_provider())
                .await
                .is_err()
        );
        assert_eq!(fixture.fake.calls(), Vec::new());
        drop(guard);
        fixture
            .engine
            .sync_provider()
            .await
            .expect("sync after mutation");
    }

    #[tokio::test]
    async fn auto_delete_removes_local_files_but_never_provider_content() {
        let mut settings = Settings::default();
        settings.provider.auto_delete = true;
        let fixture = TestEngine::with_settings(settings)
            .await
            .expect("test engine");
        let root = fixture
            .engine
            .settings()
            .download_client
            .download_path
            .clone();
        let directory = std::path::Path::new(&root).join("series").join("Example");
        tokio::fs::create_dir_all(&directory)
            .await
            .expect("create torrent directory");
        tokio::fs::write(directory.join("video.mkv"), b"downloaded data")
            .await
            .expect("write local file");
        let torrent = insert(
            &fixture,
            Some("removed"),
            "hash",
            &Torrent {
                category: Some("series".to_owned()),
                rd_name: Some("Example".to_owned()),
                rd_status: Some(TorrentStatus::Downloading),
                ..Torrent::default()
            },
        )
        .await;

        fixture.engine.sync_provider().await.expect("auto delete");

        assert!(!directory.exists());
        assert!(
            fixture
                .store
                .torrents()
                .get(torrent.torrent_id)
                .await
                .expect("query")
                .is_none()
        );
        assert!(
            !fixture
                .fake
                .calls()
                .iter()
                .any(|call| matches!(call, ProviderCall::Delete { .. }))
        );
    }

    #[tokio::test]
    async fn refresh_rate_limit_stops_sync_before_auto_delete_and_releases_the_lock() {
        let mut settings = Settings::default();
        settings.provider.auto_delete = true;
        let fixture = TestEngine::with_settings(settings)
            .await
            .expect("test engine");
        let remote = remote("42");
        fixture.fake.set_torrents(vec![remote.clone()]);
        let present = insert(&fixture, Some("42"), &remote.hash, &Torrent::default()).await;
        let missing = insert(&fixture, Some("missing"), "other-hash", &Torrent::default()).await;
        fixture
            .fake
            .queue_rate_limit(ProviderOperation::Refresh, Duration::from_secs(30));
        assert!(matches!(
            fixture
                .engine
                .sync_provider()
                .await
                .expect_err("refresh rate limit"),
            EngineError::Provider(DebridError::RateLimited { .. })
        ));
        assert_eq!(
            stored(&fixture, &missing).await.torrent_id,
            missing.torrent_id
        );
        assert_eq!(stored(&fixture, &present).await.rd_name, None);
        assert!(
            fixture
                .engine
                .rate_limit_status()
                .next_dequeue_time
                .is_some()
        );
        tokio::time::timeout(Duration::from_secs(5), fixture.engine.sync_provider())
            .await
            .expect("lock released after refresh error")
            .expect("second sync");
        assert!(
            fixture
                .store
                .torrents()
                .get(missing.torrent_id)
                .await
                .expect("query")
                .is_none()
        );
    }

    #[tokio::test]
    async fn listing_rate_limits_record_the_engine_cooldown() {
        let fixture = TestEngine::new().await.expect("test engine");
        fixture
            .fake
            .queue_rate_limit(ProviderOperation::ListTorrents, Duration::from_secs(20));
        assert!(matches!(
            fixture
                .engine
                .sync_provider()
                .await
                .expect_err("listing rate limit"),
            EngineError::Provider(DebridError::RateLimited { .. })
        ));
        let status = fixture.engine.rate_limit_status();
        assert!(status.next_dequeue_time.is_some());
        assert!(status.seconds_remaining > 0.0 && status.seconds_remaining <= 20.0);
    }

    #[tokio::test]
    async fn refresh_without_a_provider_id_preserves_the_queued_torrent() {
        let fixture = TestEngine::new().await.expect("test engine");
        let mut torrent = insert(
            &fixture,
            None,
            "queued-hash",
            &Torrent {
                rd_status: Some(TorrentStatus::Queued),
                ..Torrent::default()
            },
        )
        .await;
        let before = torrent.clone();
        let _ = fixture.fake.take_calls();

        fixture
            .engine
            .refresh_torrent(&mut torrent, None)
            .await
            .expect("queued refresh");

        assert_eq!(torrent, before);
        assert_eq!(stored(&fixture, &torrent).await, before);
        assert_eq!(fixture.fake.calls(), Vec::new());
    }
}

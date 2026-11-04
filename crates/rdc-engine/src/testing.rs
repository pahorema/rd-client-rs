//! Controllable, network-free provider and isolated engine fixtures for integration tests.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use async_trait::async_trait;
use rdc_debrid::{AvailableFile, DebridError, DebridProvider, DebridUser, RemoteTorrent};
use rdc_model::{Download, DownloadInfo, Provider, Torrent, TorrentStatus, settings::Settings};
use rdc_store::Store;
use rdc_torrent::{MagnetLink, TorrentFile};
use tempfile::TempDir;
use uuid::Uuid;

use crate::{Engine, EngineError, EngineOptions};

/// An operation whose next invocation can fail independently of other operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderOperation {
    /// List the account's torrents.
    ListTorrents,
    /// Read the account information.
    User,
    /// Add a magnet link.
    AddMagnet,
    /// Add torrent metadata.
    AddTorrentFile,
    /// Read cached files for an info hash.
    AvailableFiles,
    /// Select files for a torrent.
    SelectFiles,
    /// Delete a provider torrent.
    Delete,
    /// Resolve a restricted link.
    Unrestrict,
    /// Refresh a torrent's provider fields.
    Refresh,
    /// List downloadable files.
    DownloadInfos,
    /// Resolve a download's file name.
    FileName,
    /// Resolve a mounted file's path.
    SymlinkPath,
}

/// An attempted provider call, recorded before a queued failure is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCall {
    /// List all torrents.
    ListTorrents,
    /// Read account information.
    User,
    /// Add this exact magnet URI.
    AddMagnet(String),
    /// Add these exact metadata bytes.
    AddTorrentFile(Vec<u8>),
    /// Query this info hash.
    AvailableFiles(String),
    /// Select files on this local torrent.
    SelectFiles(Uuid),
    /// Delete the local torrent's corresponding provider entry.
    Delete {
        /// Local torrent identifier.
        torrent_id: Uuid,
        /// Provider identifier supplied by the caller.
        provider_id: Option<String>,
    },
    /// Resolve a link for this local torrent.
    Unrestrict {
        /// Local torrent identifier.
        torrent_id: Uuid,
        /// Restricted link supplied by the caller.
        link: String,
    },
    /// Refresh using either a supplied listing entry or the fake's current state.
    Refresh {
        /// Local torrent identifier.
        torrent_id: Uuid,
        /// Listing entry supplied by the caller; `None` means an individual lookup.
        remote: Option<Box<RemoteTorrent>>,
    },
    /// Query downloadable files for this local torrent.
    DownloadInfos(Uuid),
    /// Resolve this local download's name.
    FileName(Uuid),
    /// Resolve this local download's mounted path.
    SymlinkPath(Uuid),
}

#[derive(Debug, Default)]
struct FakeState {
    torrents: BTreeMap<String, RemoteTorrent>,
    additions: VecDeque<RemoteTorrent>,
    failures: BTreeMap<ProviderOperation, VecDeque<DebridError>>,
    calls: Vec<ProviderCall>,
    next_id: u64,
    user: DebridUser,
    available: BTreeMap<String, Vec<AvailableFile>>,
    selection: BTreeMap<String, Option<usize>>,
    download_infos: BTreeMap<String, Option<Vec<DownloadInfo>>>,
    unrestricted: BTreeMap<String, String>,
    file_names: BTreeMap<Uuid, String>,
    statuses: BTreeMap<String, TorrentStatus>,
}

/// A cloneable in-memory provider whose controls and call history share one state.
///
/// No method performs network I/O or starts a task. Failures are FIFO per operation,
/// and failed calls are captured without applying their normal state changes.
/// Remote listing order is deterministic (lexicographically by provider identifier).
/// Clones share state; [`with_kind`](Self::with_kind) can present that state as another provider.
#[derive(Debug, Clone)]
pub struct FakeDebridProvider {
    kind: Provider,
    state: Arc<Mutex<FakeState>>,
}

impl Default for FakeDebridProvider {
    fn default() -> Self {
        Self::new(Provider::RealDebrid)
    }
}

impl FakeDebridProvider {
    /// Creates an empty account for the chosen provider kind.
    #[must_use]
    pub fn new(kind: Provider) -> Self {
        Self {
            kind,
            state: Arc::new(Mutex::new(FakeState::default())),
        }
    }

    /// Shares this fake's state while reporting a different provider kind.
    #[must_use]
    pub fn with_kind(&self, kind: Provider) -> Self {
        Self {
            kind,
            state: Arc::clone(&self.state),
        }
    }

    /// Returns startup options injecting this fake and disabling all background jobs.
    ///
    /// Rebuilding the provider after settings changes retains the shared fake state
    /// and reports the newly requested provider kind.
    #[must_use]
    pub fn options(&self) -> EngineOptions {
        let fake = self.clone();
        EngineOptions {
            provider_factory: Arc::new(move |kind, _, _| {
                Ok(Arc::new(fake.with_kind(kind)) as Arc<dyn DebridProvider>)
            }),
            background_jobs: false,
            ..EngineOptions::default()
        }
    }

    /// Replaces all remote torrents without changing call history or failure queues.
    pub fn set_torrents(&self, torrents: Vec<RemoteTorrent>) {
        self.lock().torrents = torrents
            .into_iter()
            .map(|torrent| (torrent.id.clone(), torrent))
            .collect();
    }

    /// Inserts or replaces a remote torrent by provider identifier.
    pub fn insert_torrent(&self, torrent: RemoteTorrent) {
        self.lock().torrents.insert(torrent.id.clone(), torrent);
    }

    /// Removes a remote entry without recording a provider delete call.
    pub fn remove_torrent(&self, id: &str) {
        self.lock().torrents.remove(id);
    }

    /// Returns a snapshot of the remote account, ordered by provider identifier.
    #[must_use]
    pub fn torrents(&self) -> Vec<RemoteTorrent> {
        self.lock().torrents.values().cloned().collect()
    }

    /// Makes the next successful add use this exact remote torrent and identifier.
    ///
    /// Queued add results are shared by magnet and metadata additions. A failed
    /// attempt does not consume a queued result. An empty identifier is generated.
    pub fn queue_add(&self, torrent: RemoteTorrent) {
        self.lock().additions.push_back(torrent);
    }

    /// Queues one failure for the next invocation of the chosen operation.
    pub fn queue_failure(&self, operation: ProviderOperation, error: DebridError) {
        self.lock()
            .failures
            .entry(operation)
            .or_default()
            .push_back(error);
    }

    /// Queues a rate-limit response with a caller-controlled cooldown duration.
    pub fn queue_rate_limit(&self, operation: ProviderOperation, retry_after: Duration) {
        self.queue_failure(
            operation,
            DebridError::RateLimited {
                message: "fake provider rate limit".to_owned(),
                retry_after,
            },
        );
    }

    /// Returns all attempted calls in invocation order without clearing them.
    #[must_use]
    pub fn calls(&self) -> Vec<ProviderCall> {
        self.lock().calls.clone()
    }

    /// Removes and returns all attempted calls in invocation order.
    #[must_use]
    pub fn take_calls(&self) -> Vec<ProviderCall> {
        std::mem::take(&mut self.lock().calls)
    }

    /// Configures the account information returned by [`DebridProvider::user`].
    pub fn set_user(&self, user: DebridUser) {
        self.lock().user = user;
    }

    /// Configures cached files for an info hash; unconfigured hashes return no files.
    pub fn set_available_files(&self, hash: String, files: Vec<AvailableFile>) {
        self.lock().available.insert(hash, files);
    }

    /// Overrides file selection's response for one provider identifier.
    ///
    /// Without an override, selection uses the torrent's file filters and returns
    /// `None` until remote files are available.
    pub fn set_selection(&self, provider_id: String, result: Option<usize>) {
        self.lock().selection.insert(provider_id, result);
    }

    /// Overrides downloadable files for one provider identifier, including not-ready state.
    pub fn set_download_infos(&self, provider_id: String, infos: Option<Vec<DownloadInfo>>) {
        self.lock().download_infos.insert(provider_id, infos);
    }

    /// Configures a direct URL; links without an override are returned unchanged.
    pub fn set_unrestricted_link(&self, restricted: String, direct: String) {
        self.lock().unrestricted.insert(restricted, direct);
    }

    /// Configures the file name returned for one download identifier.
    pub fn set_file_name(&self, download_id: Uuid, name: String) {
        self.lock().file_names.insert(download_id, name);
    }

    /// Overrides the normalized status while retaining the remote's raw status text.
    pub fn set_status(&self, provider_id: String, status: TorrentStatus) {
        self.lock().statuses.insert(provider_id, status);
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state
            .lock()
            .expect("fake provider state lock poisoned")
    }
}

impl FakeState {
    fn begin(
        &mut self,
        operation: ProviderOperation,
        call: ProviderCall,
    ) -> Result<(), DebridError> {
        self.calls.push(call);
        if let Some(error) = self
            .failures
            .get_mut(&operation)
            .and_then(VecDeque::pop_front)
        {
            return Err(error);
        }
        Ok(())
    }

    fn add(&mut self, default: RemoteTorrent) -> String {
        let mut remote = self.additions.pop_front().unwrap_or(default);
        if remote.id.is_empty() {
            loop {
                self.next_id += 1;
                remote.id = format!("fake-{}", self.next_id);
                if !self.torrents.contains_key(&remote.id) {
                    break;
                }
            }
        }
        let id = remote.id.clone();
        self.torrents.insert(id.clone(), remote);
        id
    }
}

#[async_trait]
impl DebridProvider for FakeDebridProvider {
    fn kind(&self) -> Provider {
        self.kind
    }

    async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        let mut state = self.lock();
        state.begin(ProviderOperation::ListTorrents, ProviderCall::ListTorrents)?;
        Ok(state.torrents.values().cloned().collect())
    }

    async fn user(&self) -> Result<DebridUser, DebridError> {
        let mut state = self.lock();
        state.begin(ProviderOperation::User, ProviderCall::User)?;
        Ok(state.user.clone())
    }

    async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::AddMagnet,
            ProviderCall::AddMagnet(magnet.to_owned()),
        )?;
        let parsed =
            MagnetLink::parse(magnet).map_err(|error| DebridError::Provider(error.to_string()))?;
        let hash = parsed.info_hash_hex();
        Ok(state.add(RemoteTorrent {
            filename: parsed.display_name.unwrap_or_else(|| hash.clone()),
            hash,
            status: Some("queued".to_owned()),
            ..RemoteTorrent::default()
        }))
    }

    async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::AddTorrentFile,
            ProviderCall::AddTorrentFile(bytes.to_vec()),
        )?;
        let parsed =
            TorrentFile::parse(bytes).map_err(|error| DebridError::Provider(error.to_string()))?;
        let size = i64::try_from(parsed.total_size)
            .map_err(|_| DebridError::Provider("fake torrent size exceeds i64".to_owned()))?;
        Ok(state.add(RemoteTorrent {
            hash: parsed.info_hash_hex(),
            filename: parsed.name,
            bytes: size,
            original_bytes: size,
            status: Some("queued".to_owned()),
            ..RemoteTorrent::default()
        }))
    }

    async fn available_files(&self, hash: &str) -> Result<Vec<AvailableFile>, DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::AvailableFiles,
            ProviderCall::AvailableFiles(hash.to_owned()),
        )?;
        Ok(state.available.get(hash).cloned().unwrap_or_default())
    }

    async fn select_files(&self, torrent: &Torrent) -> Result<Option<usize>, DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::SelectFiles,
            ProviderCall::SelectFiles(torrent.torrent_id),
        )?;
        let id = torrent.rd_id.as_deref().unwrap_or_default();
        if let Some(result) = state.selection.get(id) {
            return Ok(*result);
        }
        let Some(files) = state
            .torrents
            .get_mut(id)
            .and_then(|remote| remote.files.as_mut())
        else {
            return Ok(None);
        };
        let selected = files
            .iter()
            .map(|file| torrent.is_downloadable(&file.path, file.bytes))
            .collect::<Result<Vec<_>, _>>()?;
        let count = selected.iter().filter(|selected| **selected).count();
        for (file, selected) in files.iter_mut().zip(selected) {
            file.selected = selected;
        }
        Ok(Some(count))
    }

    async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::Delete,
            ProviderCall::Delete {
                torrent_id: torrent.torrent_id,
                provider_id: torrent.rd_id.clone(),
            },
        )?;
        if let Some(id) = torrent.rd_id.as_deref() {
            state.torrents.remove(id);
        }
        Ok(())
    }

    async fn unrestrict(&self, torrent: &Torrent, link: &str) -> Result<String, DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::Unrestrict,
            ProviderCall::Unrestrict {
                torrent_id: torrent.torrent_id,
                link: link.to_owned(),
            },
        )?;
        Ok(state
            .unrestricted
            .get(link)
            .cloned()
            .unwrap_or_else(|| link.to_owned()))
    }

    async fn refresh(
        &self,
        torrent: &mut Torrent,
        remote: Option<&RemoteTorrent>,
    ) -> Result<(), DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::Refresh,
            ProviderCall::Refresh {
                torrent_id: torrent.torrent_id,
                remote: remote.map(|remote| Box::new(remote.clone())),
            },
        )?;
        let remote = remote.or_else(|| {
            torrent
                .rd_id
                .as_deref()
                .and_then(|id| state.torrents.get(id))
        });
        torrent.client_kind = Some(self.kind);
        let Some(remote) = remote else {
            torrent.rd_status_raw = Some("deleted".to_owned());
            torrent.rd_status = Some(TorrentStatus::Error);
            return Ok(());
        };
        torrent.rd_name = Some(
            remote
                .original_filename
                .as_ref()
                .unwrap_or(&remote.filename)
                .clone(),
        );
        torrent.rd_size = Some(if remote.bytes > 0 {
            remote.bytes
        } else {
            remote.original_bytes
        });
        torrent.rd_host.clone_from(&remote.host);
        torrent.rd_split = Some(remote.split);
        torrent.rd_progress = Some(remote.progress);
        torrent.rd_status_raw.clone_from(&remote.status);
        torrent.rd_status = Some(
            state
                .statuses
                .get(&remote.id)
                .copied()
                .unwrap_or_else(|| normalized_status(remote)),
        );
        torrent.rd_added = remote.added;
        torrent.rd_ended = remote.ended;
        torrent.rd_speed = remote.speed;
        torrent.rd_seeders = remote.seeders;
        if let Some(files) = &remote.files {
            torrent.set_files(files);
        }
        Ok(())
    }

    async fn download_infos(
        &self,
        torrent: &Torrent,
    ) -> Result<Option<Vec<DownloadInfo>>, DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::DownloadInfos,
            ProviderCall::DownloadInfos(torrent.torrent_id),
        )?;
        let id = torrent.rd_id.as_deref().unwrap_or_default();
        if let Some(infos) = state.download_infos.get(id) {
            return Ok(infos.clone());
        }
        let Some(remote) = state.torrents.get(id) else {
            return Ok(None);
        };
        let Some(files) = &remote.files else {
            return Ok(remote.links.as_ref().map(|links| {
                links
                    .iter()
                    .map(|link| DownloadInfo {
                        file_name: None,
                        restricted_link: link.clone(),
                    })
                    .collect()
            }));
        };
        let mut infos = Vec::new();
        for file in files {
            if file.selected && torrent.is_downloadable(&file.path, file.bytes)? {
                let Some(link) = &file.download_link else {
                    return Ok(None);
                };
                infos.push(DownloadInfo {
                    file_name: Some(file.path.clone()),
                    restricted_link: link.clone(),
                });
            }
        }
        Ok(Some(infos))
    }

    async fn file_name(&self, download: &Download) -> Result<String, DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::FileName,
            ProviderCall::FileName(download.download_id),
        )?;
        Ok(state
            .file_names
            .get(&download.download_id)
            .or(download.file_name.as_ref())
            .cloned()
            .unwrap_or_else(|| {
                download
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            }))
    }

    fn symlink_path(
        &self,
        torrent: &Torrent,
        download: &Download,
    ) -> Result<Option<String>, DebridError> {
        let mut state = self.lock();
        state.begin(
            ProviderOperation::SymlinkPath,
            ProviderCall::SymlinkPath(download.download_id),
        )?;
        Ok(rdc_model::paths::relative_download_path(torrent, download))
    }
}

fn normalized_status(remote: &RemoteTorrent) -> TorrentStatus {
    match remote.status.as_deref() {
        Some("queued") => TorrentStatus::Queued,
        Some("processing" | "magnet_conversion") => TorrentStatus::Processing,
        Some("waiting_files_selection") => TorrentStatus::WaitingForFileSelection,
        Some("downloading" | "compressing") => TorrentStatus::Downloading,
        Some("downloaded" | "finished" | "ready" | "completed") => TorrentStatus::Finished,
        Some("uploading" | "seeding") => TorrentStatus::Uploading,
        Some(_) => TorrentStatus::Error,
        None if remote.progress >= 100 => TorrentStatus::Finished,
        None => TorrentStatus::Queued,
    }
}

/// An engine started against a private temporary database with background jobs disabled.
///
/// Settings are seeded and persisted before startup, so no reload is needed.
/// Default download and mapped directories are replaced with a directory inside
/// `tempdir`; explicitly configured paths are retained. Keep this fixture alive
/// while using its engine or store so the temporary directory remains present.
pub struct TestEngine {
    /// The engine under test.
    pub engine: Engine,
    /// The same database handle supplied to the engine.
    pub store: Store,
    /// Owns the database and the default local download directory.
    pub tempdir: TempDir,
    /// The fake used by the default fixture provider factory.
    pub fake: FakeDebridProvider,
}

impl fmt::Debug for TestEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TestEngine")
            .field("store", &self.store)
            .field("tempdir", &self.tempdir)
            .field("fake", &self.fake)
            .finish_non_exhaustive()
    }
}

impl TestEngine {
    /// Starts an isolated engine with default settings and an empty fake account.
    ///
    /// # Errors
    /// Returns filesystem, database, or engine startup errors.
    pub async fn new() -> Result<Self, EngineError> {
        Self::with_settings(Settings::default()).await
    }

    /// Starts with persisted settings and a fake matching their configured provider kind.
    ///
    /// # Errors
    /// Returns filesystem, database, or engine startup errors.
    pub async fn with_settings(settings: Settings) -> Result<Self, EngineError> {
        let fake = FakeDebridProvider::new(settings.provider.provider);
        Self::with_provider(settings, fake).await
    }

    /// Starts with a preconfigured fake, persisting its kind into provider settings.
    ///
    /// # Errors
    /// Returns filesystem, database, or engine startup errors.
    pub async fn with_provider(
        mut settings: Settings,
        fake: FakeDebridProvider,
    ) -> Result<Self, EngineError> {
        settings.provider.provider = fake.kind();
        let options = fake.options();
        Self::with_options(settings, fake, options).await
    }

    /// Starts with custom factories and listeners while unconditionally disabling background jobs.
    ///
    /// The supplied provider factory is preserved; use `fake.options()` as the
    /// starting point when the engine should use the fixture's fake. This allows
    /// factories that return different fake accounts after a settings reload.
    ///
    /// # Errors
    /// Returns filesystem, database, or engine startup errors, including factory failures.
    pub async fn with_options(
        settings: Settings,
        fake: FakeDebridProvider,
        mut options: EngineOptions,
    ) -> Result<Self, EngineError> {
        let tempdir = tempfile::tempdir()?;
        tokio::fs::create_dir_all(tempdir.path().join("downloads")).await?;
        let mut settings = settings;
        let defaults = Settings::default();
        let download_path = tempdir
            .path()
            .join("downloads")
            .to_string_lossy()
            .into_owned();
        if settings.download_client.download_path == defaults.download_client.download_path {
            settings
                .download_client
                .download_path
                .clone_from(&download_path);
        }
        if settings.download_client.mapped_path == defaults.download_client.mapped_path {
            settings.download_client.mapped_path = download_path;
        }
        let store = Store::open(tempdir.path().join("rd-client.db")).await?;
        store.settings().seed().await?;
        let rows = settings
            .stored_rows()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<Vec<_>>();
        store.settings().update(&rows).await?;
        options.background_jobs = false;
        let engine = Engine::start(store.clone(), options).await?;
        Ok(Self {
            engine,
            store,
            tempdir,
            fake,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAGNET: &str = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=Example";

    #[tokio::test]
    async fn queued_failures_are_per_operation_and_preserve_successful_add_result() {
        let fake = FakeDebridProvider::new(Provider::TorBox);
        fake.queue_add(RemoteTorrent {
            id: "chosen".to_owned(),
            filename: "custom".to_owned(),
            ..RemoteTorrent::default()
        });
        fake.queue_rate_limit(ProviderOperation::AddMagnet, Duration::from_secs(7));
        fake.queue_failure(
            ProviderOperation::AddMagnet,
            DebridError::Provider("second failure".to_owned()),
        );
        assert_eq!(
            fake.list_torrents().await.expect("list is independent"),
            Vec::new()
        );
        let error = fake
            .add_magnet(MAGNET)
            .await
            .expect_err("first queued failure");
        assert_eq!(error.retry_after(), Some(Duration::from_secs(7)));
        assert_eq!(
            fake.add_magnet(MAGNET)
                .await
                .expect_err("second queued failure")
                .to_string(),
            "second failure"
        );
        assert_eq!(
            fake.add_magnet(MAGNET).await.expect("queued addition"),
            "chosen"
        );
        assert_eq!(fake.torrents()[0].filename, "custom");
        assert_eq!(
            fake.calls(),
            vec![
                ProviderCall::ListTorrents,
                ProviderCall::AddMagnet(MAGNET.to_owned()),
                ProviderCall::AddMagnet(MAGNET.to_owned()),
                ProviderCall::AddMagnet(MAGNET.to_owned())
            ]
        );
        assert_eq!(fake.kind(), Provider::TorBox);
    }

    #[tokio::test]
    async fn refresh_prefers_supplied_remote_and_reports_deleted_after_removal() {
        let fake = FakeDebridProvider::new(Provider::AllDebrid);
        fake.insert_torrent(RemoteTorrent {
            id: "remote".to_owned(),
            filename: "stored".to_owned(),
            ..RemoteTorrent::default()
        });
        let mut torrent = Torrent {
            rd_id: Some("remote".to_owned()),
            ..Torrent::default()
        };
        let supplied = RemoteTorrent {
            id: "remote".to_owned(),
            filename: "listing".to_owned(),
            status: Some("downloaded".to_owned()),
            ..RemoteTorrent::default()
        };
        fake.refresh(&mut torrent, Some(&supplied))
            .await
            .expect("refresh from listing");
        assert_eq!(torrent.rd_name.as_deref(), Some("listing"));
        assert_eq!(torrent.client_kind, Some(Provider::AllDebrid));
        assert_eq!(torrent.rd_status, Some(TorrentStatus::Finished));
        fake.remove_torrent("remote");
        fake.refresh(&mut torrent, None)
            .await
            .expect("refresh missing entry");
        assert_eq!(torrent.rd_status_raw.as_deref(), Some("deleted"));
        assert_eq!(
            fake.take_calls(),
            vec![
                ProviderCall::Refresh {
                    torrent_id: torrent.torrent_id,
                    remote: Some(Box::new(supplied))
                },
                ProviderCall::Refresh {
                    torrent_id: torrent.torrent_id,
                    remote: None
                }
            ]
        );
        assert_eq!(fake.calls(), Vec::new());
    }

    #[tokio::test]
    async fn failed_delete_retains_remote_and_successful_delete_removes_it() {
        let fake = FakeDebridProvider::default();
        fake.insert_torrent(RemoteTorrent {
            id: "remote".to_owned(),
            ..RemoteTorrent::default()
        });
        let torrent = Torrent {
            rd_id: Some("remote".to_owned()),
            ..Torrent::default()
        };
        fake.queue_failure(
            ProviderOperation::Delete,
            DebridError::Provider("cannot delete".to_owned()),
        );
        assert!(fake.delete(&torrent).await.is_err());
        assert_eq!(fake.torrents().len(), 1);
        fake.delete(&torrent).await.expect("delete after failure");
        assert_eq!(fake.torrents(), Vec::new());
        assert_eq!(fake.calls().len(), 2);
    }

    #[tokio::test]
    async fn custom_provider_factory_receives_persisted_configuration() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut settings = Settings::default();
        settings.provider.provider = Provider::Premiumize;
        settings.provider.api_key = "seeded-test-key".to_owned();
        let fake = FakeDebridProvider::new(Provider::Premiumize);
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = fake.clone();
        let factory_calls = Arc::clone(&calls);
        let mut options = fake.options();
        options.background_jobs = true;
        options.provider_factory = Arc::new(move |kind, config, _| {
            assert_eq!(kind, Provider::Premiumize);
            assert_eq!(config.api_key, "seeded-test-key");
            factory_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(provider.clone()) as Arc<dyn DebridProvider>)
        });
        let fixture = TestEngine::with_options(settings, fake, options)
            .await
            .expect("custom factory startup");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.fake.calls(), Vec::new());
    }

    #[tokio::test]
    async fn startup_persists_settings_and_does_not_call_provider() {
        let mut settings = Settings::default();
        settings.provider.provider = Provider::Highway;
        settings.provider.auto_import = true;
        settings.provider.auto_delete = true;
        let fixture = TestEngine::with_settings(settings)
            .await
            .expect("isolated engine");
        assert_eq!(
            fixture.engine.settings().provider.provider,
            Provider::Highway
        );
        assert!(
            fixture
                .store
                .settings()
                .load()
                .await
                .expect("stored settings")
                .provider
                .auto_import
        );
        assert!(
            std::path::Path::new(&fixture.engine.settings().download_client.download_path)
                .starts_with(fixture.tempdir.path())
        );
        assert_eq!(
            fixture.fake.calls(),
            Vec::new(),
            "startup must not contact the provider"
        );
    }
}

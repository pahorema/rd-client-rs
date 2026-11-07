//! Configured host engines and the progress/completion wrapper used by the lifecycle runner.

use crate::{DownloadRequest, Engine, EngineError};
use async_trait::async_trait;
use rdc_download::{
    Aria2Config, Aria2Download, Aria2Request, Completion, CompletionSender, DownloadEngine,
    DownloadStationConfig, DownloadStationDownload, DownloadStationRequest, HttpDownload, Progress,
    SourceLookup, StartError, SymlinkDownload, SymlinkRequest, completion_channel,
};
use rdc_model::{Download, DownloadClient, Provider, RemoteFile, Torrent, paths};
use std::{
    io,
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{fs, sync::watch};
use tokio_util::sync::CancellationToken;

impl Engine {
    /// Where a download is written, with its parent directories created and checked.
    pub(crate) async fn output_paths(
        &self,
        torrent: &Torrent,
        download: &Download,
    ) -> Result<(PathBuf, PathBuf, String), EngineError> {
        let settings = self.settings();
        let root = Path::new(&settings.download_client.download_path);
        let paths = output_location(root, torrent, &torrent.files(), download)?;
        create_parent_directories(root, &paths.1).await?;
        Ok(paths)
    }

    pub(crate) async fn prepare_download(
        &self,
        torrent: &Torrent,
        download: &Download,
    ) -> Result<Arc<dyn DownloadEngine>, EngineError> {
        let (_, output, relative) = self.output_paths(torrent, download).await?;
        if torrent.download_client != DownloadClient::Symlink {
            match fs::symlink_metadata(&output).await {
                Ok(metadata) if metadata.is_symlink() => return Err(invalid_path()),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let backend = self
            .configured_download(torrent, download, output.clone(), relative)
            .await?;
        let expected = if torrent.download_client == DownloadClient::Internal {
            paths::expected_file_size(torrent, download).and_then(|size| u64::try_from(size).ok())
        } else {
            None
        };
        let fallback = if torrent.downloads.len() == 1 {
            u64::try_from(torrent.rd_size.unwrap_or_default()).unwrap_or_default()
        } else {
            0
        };
        // A stale file from an earlier attempt is replaced, but a file owned by an adopted
        // remote transfer is kept.
        let remove_target = match torrent.download_client {
            DownloadClient::Internal => true,
            DownloadClient::Aria2c | DownloadClient::DownloadStation => download
                .remote_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty()),
            DownloadClient::Symlink => false,
        };
        Ok(Arc::new(Transfer::new(
            backend,
            output,
            expected,
            fallback,
            remove_target,
        )))
    }

    pub(crate) async fn configured_download(
        &self,
        torrent: &Torrent,
        download: &Download,
        output: PathBuf,
        relative: String,
    ) -> Result<Arc<dyn DownloadEngine>, EngineError> {
        let settings = self.settings();
        if let Some(factory) = self.download_factory() {
            return factory
                .create(DownloadRequest {
                    download: download.clone(),
                    torrent: torrent.clone(),
                    settings,
                    local_path: output,
                    relative_path: relative,
                    internal_config: self.internal_download_config(),
                })
                .await;
        }
        let url = download
            .link
            .as_deref()
            .filter(|link| !link.trim().is_empty())
            .ok_or_else(|| EngineError::Message("Invalid download link".to_owned()))?;
        let config = &settings.download_client;
        let category = torrent
            .category
            .as_deref()
            .map(paths::remove_invalid_file_name_chars)
            .filter(|category| !category.trim().is_empty() && category != ".");
        let backend: Arc<dyn DownloadEngine> = match torrent.download_client {
            DownloadClient::Internal => Arc::new(HttpDownload::new(
                url,
                output,
                self.internal_download_config(),
            )),
            DownloadClient::Aria2c => Arc::new(Aria2Download::new(
                Aria2Request {
                    url: url.to_owned(),
                    file_path: output,
                    relative_path: relative,
                    category,
                    gid: download.remote_id.clone(),
                },
                Aria2Config {
                    url: config.aria2c_url.clone(),
                    secret: Some(config.aria2c_secret.clone()),
                    download_path: config.aria2c_download_path.clone(),
                },
            )),
            DownloadClient::DownloadStation => Arc::new(DownloadStationDownload::new(
                DownloadStationRequest {
                    url: url.to_owned(),
                    file_path: output,
                    relative_path: relative,
                    category,
                    gid: download.remote_id.clone(),
                },
                DownloadStationConfig {
                    url: Some(config.download_station_url.clone()),
                    username: config.download_station_username.clone(),
                    password: config.download_station_password.clone(),
                    download_path: config.download_station_download_path.clone(),
                },
                self.inner.synology_sessions.clone(),
            )),
            DownloadClient::Symlink => {
                let provider = self.provider();
                let relative = if matches!(
                    torrent.client_kind,
                    Some(Provider::AllDebrid | Provider::DebridLink | Provider::Highway)
                ) {
                    provider
                        .symlink_path(torrent, download)
                        .map_err(|error| {
                            self.record_provider_error(&error);
                            EngineError::Provider(error)
                        })?
                        .ok_or_else(invalid_path)?
                } else {
                    relative
                };
                if Path::new(&relative)
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
                {
                    return Err(invalid_path());
                }
                Arc::new(SymlinkDownload::new(SymlinkRequest {
                    mount_path: config.rclone_mount_path.clone(),
                    relative_path: relative,
                    destination: output,
                    lookup: if torrent.client_kind == Some(Provider::AllDebrid) {
                        SourceLookup::DirectThenSearch
                    } else {
                        SourceLookup::Search
                    },
                }))
            }
        };
        Ok(backend)
    }
}

/// The destination directory, the output file and its path relative to the destination of a
/// download of `torrent`, whose parsed provider files are `files`. Touches no filesystem.
pub(crate) fn output_location(
    root: &Path,
    torrent: &Torrent,
    files: &[RemoteFile],
    download: &Download,
) -> Result<(PathBuf, PathBuf, String), EngineError> {
    if root.as_os_str().is_empty() {
        return Err(invalid_path());
    }
    let name = torrent
        .rd_name
        .as_deref()
        .map(paths::remove_invalid_file_name_chars)
        .filter(|name| !name.trim().is_empty() && name != ".")
        .ok_or_else(invalid_path)?;
    let category = torrent
        .category
        .as_deref()
        .map(paths::remove_invalid_file_name_chars)
        .filter(|name| !name.trim().is_empty() && name != ".");
    let destination = category
        .as_ref()
        .map_or_else(|| root.to_path_buf(), |category| root.join(category));
    let safe_download = Download {
        link: download.link.clone(),
        file_name: paths::file_name(download),
        ..Download::default()
    };
    if safe_download
        .file_name
        .as_deref()
        .is_none_or(|name| name.trim().is_empty() || name == ".")
    {
        return Err(invalid_path());
    }
    let relative =
        paths::relative_download_path_in(&name, files, &safe_download).ok_or_else(invalid_path)?;
    let output = destination.join(&relative);
    let inside = output.strip_prefix(root).map_err(|_| invalid_path())?;
    if inside
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_path());
    }
    Ok((destination, output, relative))
}

/// Creates the directories between `root` and `output`, refusing symlinked or non-directory
/// parents.
async fn create_parent_directories(root: &Path, output: &Path) -> Result<(), EngineError> {
    let inside = output.strip_prefix(root).map_err(|_| invalid_path())?;
    fs::create_dir_all(root).await?;
    let mut parent = root.to_path_buf();
    let parent_relative = inside.parent().ok_or_else(invalid_path)?;
    for component in parent_relative.components() {
        parent.push(component);
        match fs::symlink_metadata(&parent).await {
            Ok(metadata) if metadata.is_symlink() || !metadata.is_dir() => {
                return Err(invalid_path());
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(&parent).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        let metadata = fs::symlink_metadata(&parent).await?;
                        if metadata.is_symlink() || !metadata.is_dir() {
                            return Err(invalid_path());
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn invalid_path() -> EngineError {
    EngineError::Message("Invalid download path".to_owned())
}

#[derive(Debug)]
struct Transfer {
    shared: Arc<TransferState>,
}

#[derive(Debug)]
struct TransferState {
    backend: Arc<dyn DownloadEngine>,
    output: PathBuf,
    expected: Option<u64>,
    fallback: u64,
    remove_target: bool,
    started: AtomicBool,
    cancel: CancellationToken,
    progress: watch::Sender<Progress>,
    completion: CompletionSender,
}

impl Transfer {
    fn new(
        backend: Arc<dyn DownloadEngine>,
        output: PathBuf,
        expected: Option<u64>,
        fallback: u64,
        remove_target: bool,
    ) -> Self {
        Self {
            shared: Arc::new(TransferState {
                backend,
                output,
                expected,
                fallback,
                remove_target,
                started: AtomicBool::new(false),
                cancel: CancellationToken::new(),
                progress: watch::Sender::new(Progress::default()),
                completion: completion_channel().0,
            }),
        }
    }
}

impl TransferState {
    fn publish(&self) {
        let mut progress = *self.backend.progress().borrow();
        if progress.bytes_total == 0 {
            progress.bytes_total = self.fallback;
        }
        if self.completion.is_completed() {
            progress.speed = 0;
        }
        self.progress.send_replace(progress);
    }
    async fn fail(&self, message: String) -> StartError {
        self.backend.cancel().await;
        self.completion.complete(Err(message.clone()));
        self.publish();
        StartError::new(message)
    }
    async fn monitor(self: Arc<Self>) {
        let mut completion = self.backend.completion();
        let mut progress = self.backend.progress();
        let mut open = true;
        let result = loop {
            self.publish();
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => break Err(rdc_download::CANCELLED_MESSAGE.to_owned()),
                outcome = completion.wait() => break outcome,
                changed = progress.changed(), if open => open = changed.is_ok(),
            }
        };
        let result = result.and_then(|()| {
            self.expected.map_or(Ok(()), |expected| {
                rdc_download::verify_file_size(&self.output, expected)
                    .map_err(|error| error.to_string())
            })
        });
        self.completion.complete(result);
        self.publish();
    }
}

#[async_trait]
impl DownloadEngine for Transfer {
    async fn start(&self) -> Result<String, StartError> {
        let state = &self.shared;
        if state.started.swap(true, Ordering::AcqRel) {
            return Err(StartError::new("The download was already started"));
        }
        if state.cancel.is_cancelled() {
            return Err(state.fail(rdc_download::CANCELLED_MESSAGE.to_owned()).await);
        }
        if state.remove_target
            && let Err(error) = fs::remove_file(&state.output).await
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(state.fail(error.to_string()).await);
        }
        // Never drop a backend start halfway: a remote job it created would be left behind.
        // `cancel` signals the backend, which stops starting and removes what it created.
        let result = state.backend.start().await;
        if state.cancel.is_cancelled() {
            return Err(state.fail(rdc_download::CANCELLED_MESSAGE.to_owned()).await);
        }
        match result {
            Ok(id) if !id.trim().is_empty() => {
                tokio::spawn(state.clone().monitor());
                Ok(id)
            }
            Ok(_) => Err(state
                .fail("No remote ID received from download client".to_owned())
                .await),
            Err(error) => Err(state.fail(error.to_string()).await),
        }
    }
    fn progress(&self) -> watch::Receiver<Progress> {
        self.shared.publish();
        self.shared.progress.subscribe()
    }
    fn completion(&self) -> Completion {
        self.shared.completion.subscribe()
    }
    async fn cancel(&self) {
        self.shared.cancel.cancel();
        self.shared.backend.cancel().await;
        self.shared
            .completion
            .complete(Err(rdc_download::CANCELLED_MESSAGE.to_owned()));
        self.shared.publish();
    }
    async fn pause(&self) {
        self.shared.backend.pause().await;
    }
    async fn resume(&self) {
        self.shared.backend.resume().await;
    }
}

#[cfg(test)]
#[path = "download_runtime_tests.rs"]
mod tests;

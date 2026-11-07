//! The Download Station engine: hands a download to a Synology NAS and follows the task.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use rdc_synology::{Credentials, Error, Session, SessionProvider, Task, TaskStatus};
use tokio::sync::watch;

use crate::engine::{CANCELLED_MESSAGE, Completion, DownloadEngine, Outcome, Progress, StartError};
use crate::remote::{Polled, Signals, Timing, remote_parent, remote_path, spawn_polling};

/// Attempts to find or create the task before the start fails.
const START_ATTEMPTS: u32 = 5;

/// Where the Download Station is and how rd-client signs in.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct DownloadStationConfig {
    /// DSM URL, for example `http://nas:5000`.
    pub url: Option<String>,
    /// Account name.
    pub username: Option<String>,
    /// Account password.
    pub password: Option<String>,
    /// Root folder of the downloads, starting with a shared folder (`/Media/Downloads`); it
    /// also becomes the account's default destination when it has none. Blank means the
    /// account's default destination is the root.
    pub download_path: Option<String>,
}

impl fmt::Debug for DownloadStationConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadStationConfig")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("download_path", &self.download_path)
            .finish()
    }
}

/// What to download and where.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DownloadStationRequest {
    /// Link Download Station downloads.
    pub url: String,
    /// Local path of the finished file; only checked for existence, never written.
    pub file_path: PathBuf,
    /// Path of the file below the download folder (`<torrent>/<sub dirs>/<file>`).
    pub relative_path: String,
    /// Category folder between the root and the relative path.
    pub category: Option<String>,
    /// Task id of an earlier start; a task with this id is adopted.
    pub gid: Option<String>,
}

/// Downloads a file with Synology Download Station.
///
/// [`start`](DownloadEngine::start) signs in through the shared [`SessionProvider`], adopts
/// the task of [`DownloadStationRequest::gid`] when it still exists, and otherwise looks the
/// link up among the tasks or creates its destination folder and a task for it (up to five
/// attempts). It returns the task id. Calls failing because the session expired sign in again
/// and are retried once.
///
/// The engine then reads the task every second: a task error or a captcha request deletes the
/// task and fails; a finished task (or one with every byte downloaded) succeeds as soon as the
/// local file exists (looked for ten times, about 45 s); a task that no longer exists fails, as
/// does any other call DSM rejects, with its error; unreachable DSMs are asked again.
/// Pausing and resuming pause and resume the task; cancelling deletes it.
#[derive(Debug)]
pub struct DownloadStationDownload {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    request: DownloadStationRequest,
    config: DownloadStationConfig,
    sessions: SessionProvider,
    timing: Timing,
    signals: Signals,
    credentials: OnceLock<Credentials>,
    session: Mutex<Option<Session>>,
    remote_path: OnceLock<String>,
    gid: Mutex<Option<String>>,
}

impl DownloadStationDownload {
    /// Prepares the download described by `request` on the Download Station of `config`,
    /// signing in through `sessions` (shared by every Download Station download).
    pub fn new(
        request: DownloadStationRequest,
        config: DownloadStationConfig,
        sessions: SessionProvider,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                request,
                config,
                sessions,
                timing: Timing::default(),
                signals: Signals::new(),
                credentials: OnceLock::new(),
                session: Mutex::new(None),
                remote_path: OnceLock::new(),
                gid: Mutex::new(None),
            }),
        }
    }

    /// Replaces the wait between two looks at the task (one second by default).
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.shared_mut().timing.poll_interval = interval;
        self
    }

    /// Replaces the wait unit of the retry loops (one second by default): attempt `n` of the
    /// start and check `n` for the local file wait `n` units.
    #[must_use]
    pub fn with_retry_step(mut self, step: Duration) -> Self {
        self.shared_mut().timing.retry_step = step;
        self
    }

    fn shared_mut(&mut self) -> &mut Shared {
        Arc::get_mut(&mut self.shared).expect("configured before it is shared")
    }
}

impl Shared {
    fn gid(&self) -> Option<String> {
        self.gid.lock().expect("gid lock poisoned").clone()
    }

    fn set_gid(&self, gid: &str) {
        *self.gid.lock().expect("gid lock poisoned") = Some(gid.to_owned());
    }

    fn stored_session(&self) -> Option<Session> {
        self.session.lock().expect("session lock poisoned").clone()
    }

    async fn fresh_session(&self) -> Result<Session, Error> {
        let credentials = self
            .credentials
            .get()
            .ok_or(Error::InvalidArgument("the download was not started"))?;
        let session = self.sessions.session(credentials).await?;
        *self.session.lock().expect("session lock poisoned") = Some(session.clone());
        Ok(session)
    }

    /// Runs `operation`; when the session expired, signs in again and retries it once.
    async fn with_session<T, F, Fut>(&self, operation: F) -> Result<T, Error>
    where
        F: Fn(Session) -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    {
        let session = match self.stored_session() {
            Some(session) => session,
            None => self.fresh_session().await?,
        };
        match operation(session.clone()).await {
            Err(error) if error.is_session_error() => {
                tracing::debug!("Download Station session expired ({error}), signing in again");
                self.sessions.invalidate(Some(&session)).await;
                operation(self.fresh_session().await?).await
            }
            result => result,
        }
    }

    async fn start(&self) -> Result<String, StartError> {
        let fail = |message: String| self.signals.fail(message);
        let Some(url) = self.config.url.clone() else {
            return Err(fail(
                "No URL specified for Synology download station".to_owned(),
            ));
        };
        let (Some(username), Some(password)) =
            (self.config.username.clone(), self.config.password.clone())
        else {
            return Err(fail(
                "No username/password specified for Synology download station".to_owned(),
            ));
        };
        let credentials = self
            .credentials
            .get_or_init(|| Credentials::new(url, username.clone(), password));
        let session = self.sessions.session(credentials).await.map_err(|error| {
            fail(format!(
                "DownloadStation login failed for user '{username}': {error}. If 2-step \
                 verification is enabled on this Synology account, use a dedicated service \
                 account without 2FA."
            ))
        })?;
        *self.session.lock().expect("session lock poisoned") = Some(session);
        let root = match self.config.download_path.as_deref() {
            Some(root) if !root.trim().is_empty() => {
                self.ensure_default_destination(root).await;
                Some(root.to_owned())
            }
            _ => {
                let config = self
                    .with_session(|session| async move { session.server_config().await })
                    .await
                    .map_err(|error| fail(error.to_string()))?;
                config.default_destination
            }
        };
        let request = &self.request;
        let remote = root
            .map(|root| remote_path(&root, request.category.as_deref(), &request.relative_path));
        let folder = remote.as_deref().and_then(remote_parent).map(|parent| {
            let parent = parent.replace('\\', "/");
            if parent.starts_with('/') {
                parent
            } else {
                format!("/{parent}")
            }
        });
        let (Some(remote), Some(folder)) = (remote, folder) else {
            return Err(fail(format!(
                "Invalid file path {}",
                request.file_path.display()
            )));
        };
        self.remote_path.get_or_init(|| remote);
        let gid = self.adopt_or_create(&folder).await?;
        self.set_gid(&gid);
        if self.signals.cancel.is_cancelled() {
            self.delete().await;
            return Err(StartError::new(CANCELLED_MESSAGE));
        }
        Ok(gid)
    }

    async fn adopt_or_create(&self, folder: &str) -> Result<String, StartError> {
        if let Some(gid) = self.request.gid.as_deref()
            && self.task(gid).await.is_some()
        {
            tracing::debug!("Download Station task {gid} already exists; adopting it");
            return Ok(gid.to_owned());
        }
        let mut attempt = 0;
        while attempt < START_ATTEMPTS {
            if let Some(gid) = self.gid_from_uri().await {
                return Ok(gid);
            }
            match self.create(folder).await {
                Ok(Some(gid)) => return Ok(gid),
                Ok(None) => tracing::error!("Download Station task not found after creating it"),
                Err(error) => tracing::error!("Error creating a Download Station task: {error}"),
            }
            attempt += 1;
            if !self.signals.sleep(self.timing.retry_step * attempt).await {
                return Err(StartError::new(CANCELLED_MESSAGE));
            }
        }
        Err(self.signals.fail("Unable to download file".to_owned()))
    }

    /// Creates the destination `folder` and a task saving into it; returns its id.
    async fn create(&self, folder: &str) -> Result<Option<String>, Error> {
        match self
            .with_session(|session| async move { session.create_folder(folder).await })
            .await
        {
            Ok(_) => tracing::debug!("Ensured the Download Station folder {folder}"),
            Err(error @ Error::Api(_)) => tracing::warn!(
                "Could not create the Download Station destination folder '{folder}' ({error}). \
                 The Synology account needs File Station permission with read/write access to \
                 that path."
            ),
            Err(error) => return Err(error),
        }
        let url = self.request.url.as_str();
        let destination = folder.trim_start_matches('/');
        let ids = self
            .with_session(
                |session| async move { session.create_task(url, Some(destination)).await },
            )
            .await?;
        match ids.into_iter().next() {
            Some(gid) => Ok(Some(gid)),
            None => Ok(self.gid_from_uri().await),
        }
    }

    /// Makes `root` the account's default destination when it has none, best-effort:
    /// without one Download Station leaves every task waiting.
    async fn ensure_default_destination(&self, root: &str) {
        let result = self
            .with_session(|session| async move {
                let mut config = session.server_config().await?;
                if config
                    .default_destination
                    .as_deref()
                    .is_some_and(|destination| !destination.trim().is_empty())
                {
                    return Ok(false);
                }
                config.default_destination = Some(root.to_owned());
                session.set_server_config(&config).await?;
                Ok(true)
            })
            .await;
        match result {
            Ok(true) => tracing::info!(
                "Set the Download Station default destination to '{root}' for this account"
            ),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                "Could not set the Download Station default destination automatically ({error}). \
                 If downloads stay in 'Waiting', sign into Download Station as this account and \
                 set a default destination under Settings > BT/HTTP/FTP/NZB > Location."
            ),
        }
    }

    /// The task `gid`, or `None` when it cannot be read.
    async fn task(&self, gid: &str) -> Option<Task> {
        match self
            .with_session(|session| async move { session.task(gid).await })
            .await
        {
            Ok(task) => Some(task),
            Err(error) => {
                tracing::debug!("Unable to get Download Station task {gid}: {error}");
                None
            }
        }
    }

    /// The id of the task downloading the request's link, if any.
    async fn gid_from_uri(&self) -> Option<String> {
        let url = self.request.url.as_str();
        match self
            .with_session(|session| async move { session.list_tasks().await })
            .await
        {
            Ok(tasks) => tasks
                .into_iter()
                .find(|task| task.uri() == Some(url))
                .map(|task| task.id),
            Err(error) => {
                tracing::debug!("Unable to list Download Station tasks for {url}: {error}");
                None
            }
        }
    }

    /// Deletes the task (looked up by link when its id is unknown), ignoring failures.
    async fn delete(&self) {
        let gid = match self.gid() {
            Some(gid) => gid,
            None => match self.gid_from_uri().await {
                Some(gid) => gid,
                None => return,
            },
        };
        let result = self
            .with_session(|session| {
                let gid = gid.clone();
                async move { session.delete_task(&gid).await }
            })
            .await;
        if let Err(error) = result {
            tracing::debug!("Ignoring failed removal of Download Station task {gid}: {error}");
        }
    }
}

/// Whether Download Station is done downloading the task; finished torrents settle into
/// seeding states.
fn is_complete(task: &Task) -> bool {
    matches!(
        task.status,
        TaskStatus::Finished
            | TaskStatus::Downloaded
            | TaskStatus::Seeding
            | TaskStatus::PreSeeding
    ) || (task.size > 0 && task.size_downloaded() >= task.size)
}

#[async_trait]
impl Polled for Shared {
    fn signals(&self) -> &Signals {
        &self.signals
    }

    async fn poll(&self) -> Option<Outcome> {
        let gid = self.gid()?;
        let task = match self
            .with_session(|session| {
                let gid = gid.clone();
                async move { session.task(&gid).await }
            })
            .await
        {
            Ok(task) => task,
            Err(error) if error.is_task_not_found() => {
                return Some(Err("Task not found".to_owned()));
            }
            // DSM answered, so this is no connection blip that the next poll could outlast
            // (signing in again after a session error already failed, for example): fail with
            // the cause rather than follow a task that cannot be read.
            Err(error @ Error::Api(_)) => {
                return Some(Err(format!("DownloadStation error: {error}")));
            }
            Err(error) => {
                tracing::warn!("Unable to read Download Station task {gid}: {error}");
                return None;
            }
        };
        if task.error_detail().is_some() || task.status == TaskStatus::CaptchaNeeded {
            let detail = task
                .error_detail()
                .map_or_else(|| task.status.to_string(), str::to_owned);
            self.delete().await;
            return Some(Err(format!("DownloadStation error: {detail}")));
        }
        if !is_complete(&task) {
            self.signals.report(Progress {
                bytes_done: task.size_downloaded(),
                bytes_total: task.size,
                speed: task.speed_download(),
            });
            return None;
        }
        let file_path = &self.request.file_path;
        let found = self
            .signals
            .wait_for_file(file_path, self.timing.retry_step)
            .await?;
        Some(if found {
            Ok(())
        } else {
            Err(format!(
                "DownloadStation reported the download finished, but no file was found at {} \
                 (DownloadStation saved to {}). Check that the Download path, the \
                 DownloadStation Download Path, and the container bind mount all point at the \
                 same folder.",
                file_path.display(),
                self.remote_path.get().map_or("", String::as_str)
            ))
        })
    }
}

#[async_trait]
impl DownloadEngine for DownloadStationDownload {
    async fn start(&self) -> Result<String, StartError> {
        self.shared.signals.begin()?;
        let gid = self.shared.start().await?;
        spawn_polling(Arc::clone(&self.shared), self.shared.timing.poll_interval);
        Ok(gid)
    }

    fn progress(&self) -> watch::Receiver<Progress> {
        self.shared.signals.progress.subscribe()
    }

    fn completion(&self) -> Completion {
        self.shared.signals.completion.subscribe()
    }

    /// Stops following the task and deletes it from Download Station.
    async fn cancel(&self) {
        self.shared.signals.cancel();
        if self.shared.credentials.get().is_some() {
            self.shared.delete().await;
        }
    }

    async fn pause(&self) {
        let Some(gid) = self.shared.gid() else {
            return;
        };
        let id = gid.as_str();
        let result = self
            .shared
            .with_session(|session| async move { session.pause_task(id).await })
            .await;
        if let Err(error) = result {
            tracing::debug!("Ignoring failed pause of Download Station task {gid}: {error}");
        }
    }

    async fn resume(&self) {
        let Some(gid) = self.shared.gid() else {
            return;
        };
        let id = gid.as_str();
        let result = self
            .shared
            .with_session(|session| async move { session.resume_task(id).await })
            .await;
        if let Err(error) = result {
            tracing::debug!("Ignoring failed resume of Download Station task {gid}: {error}");
        }
    }
}

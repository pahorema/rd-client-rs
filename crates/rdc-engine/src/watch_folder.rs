//! Imports stable top-level torrent and magnet files and archives each attempted import.

use std::fmt::Display;
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fs4::fs_std::FileExt;
use thiserror::Error;
use tokio::fs;
use tokio::sync::watch;
use tokio::time::{self, Instant};
use tokio_util::sync::CancellationToken;

/// The minimum interval between scan starts, including when watching is disabled.
const MIN_INTERVAL: Duration = Duration::from_secs(10);
/// A quiet observation window catches writers that do not use advisory file locks.
const STABILITY_WINDOW: Duration = Duration::from_millis(100);
/// Persistent failures must not indefinitely increase the retry delay.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Paths and scheduling for the watch-folder importer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchFolderConfig {
    /// Directory to scan non-recursively; an empty path disables importing.
    pub path: PathBuf,
    /// Successful imports go here, or to `path/processed` when absent or empty.
    pub processed_path: Option<PathBuf>,
    /// Failed reads, imports, or processed moves go here, or to `path/error`.
    pub error_path: Option<PathBuf>,
    /// Requested start-to-start interval, clamped to at least ten seconds.
    pub interval: Duration,
}

impl Default for WatchFolderConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            processed_path: None,
            error_path: None,
            interval: MIN_INTERVAL,
        }
    }
}

impl WatchFolderConfig {
    /// Returns the effective minimum interval between scan starts.
    #[must_use]
    pub fn scan_interval(&self) -> Duration {
        self.interval.max(MIN_INTERVAL)
    }

    fn destination(&self, success: bool) -> PathBuf {
        let (configured, fallback) = if success {
            (&self.processed_path, "processed")
        } else {
            (&self.error_path, "error")
        };
        configured
            .as_ref()
            .filter(|path| !path.as_os_str().is_empty())
            .cloned()
            .unwrap_or_else(|| self.path.join(fallback))
    }
}

/// Payload passed to the importer; magnet contents retain all whitespace.
#[derive(Debug, PartialEq, Eq)]
pub enum WatchInput {
    /// Complete torrent file bytes, without parsing or enrichment.
    Torrent(Vec<u8>),
    /// Complete UTF-8 magnet file text, without trimming.
    Magnet(String),
}

/// Counts for one scan, including a partially completed cancelled scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Imports moved successfully to the processed directory.
    pub processed: usize,
    /// Failed files moved successfully to the error directory.
    pub failed: usize,
    /// Supported files deferred because they were locked or changing.
    pub skipped: usize,
}

/// A scan-level failure; per-file failures are quarantined whenever possible.
#[derive(Debug, Error)]
pub enum WatchFolderError {
    /// A directory operation or a move into the error directory failed.
    #[error("watch-folder operation failed for {path}: {source}")]
    Io {
        /// Path associated with the failed operation.
        path: PathBuf,
        /// Filesystem error that prevented progress.
        #[source]
        source: io::Error,
    },
    /// A blocking filesystem worker could not finish.
    #[error("watch-folder filesystem worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
}

fn io_error(path: &Path, source: io::Error) -> WatchFolderError {
    WatchFolderError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputKind {
    Torrent,
    Magnet,
}

fn input_kind(path: &Path) -> Option<InputKind> {
    let extension = path.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("torrent") {
        Some(InputKind::Torrent)
    } else if extension.eq_ignore_ascii_case("magnet") {
        Some(InputKind::Magnet)
    } else {
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl Fingerprint {
    fn new(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }
}

// Keeping the file alive keeps the advisory lock through the import operation.
struct LockedInput {
    payload: WatchInput,
    lock: File,
}

fn is_lock_conflict(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        return true;
    }
    false
}

async fn read_stable(
    path: &Path,
    kind: InputKind,
    cancel: &CancellationToken,
) -> Result<Option<LockedInput>, WatchFolderError> {
    let owned_path = path.to_path_buf();
    let opened = tokio::task::spawn_blocking(move || {
        let file = match File::open(&owned_path) {
            Ok(file) => file,
            Err(error) if is_lock_conflict(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        let fingerprint = Fingerprint::new(&file.metadata()?);
        Ok::<_, io::Error>(Some((file, fingerprint)))
    })
    .await?
    .map_err(|error| io_error(path, error))?;
    let Some((mut file, initial)) = opened else {
        return Ok(None);
    };
    // Observe before locking: Windows enforces the lock, which would stall a writer
    // that ignores locks and make its unfinished file look stable.
    tokio::select! {
        biased;
        () = cancel.cancelled() => return Ok(None),
        () = time::sleep(STABILITY_WINDOW) => {}
    }
    let owned_path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let current = Fingerprint::new(&std::fs::metadata(&owned_path)?);
        if current != initial || Fingerprint::new(&file.metadata()?) != initial {
            return Ok(None);
        }
        let locked = match FileExt::try_lock_exclusive(&file) {
            Ok(locked) => locked,
            Err(error) if is_lock_conflict(&error) => false,
            Err(error) => return Err(error),
        };
        if !locked {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if Fingerprint::new(&file.metadata()?) != initial
            || Fingerprint::new(&std::fs::metadata(&owned_path)?) != initial
        {
            return Ok(None);
        }
        let payload = match kind {
            InputKind::Torrent => WatchInput::Torrent(bytes),
            InputKind::Magnet => WatchInput::Magnet(
                String::from_utf8(bytes)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            ),
        };
        Ok::<_, io::Error>(Some(LockedInput {
            payload,
            lock: file,
        }))
    })
    .await?
    .map_err(|error| io_error(path, error))
}

async fn move_replacing(source: &Path, directory: &Path) -> Result<(), WatchFolderError> {
    fs::create_dir_all(directory)
        .await
        .map_err(|error| io_error(directory, error))?;
    let name = source.file_name().ok_or_else(|| {
        io_error(
            source,
            io::Error::new(io::ErrorKind::InvalidInput, "missing file name"),
        )
    })?;
    let target = directory.join(name);
    match fs::symlink_metadata(&target).await {
        Ok(_) => {
            // A configured destination may accidentally name the watch directory itself.
            if fs::canonicalize(source)
                .await
                .map_err(|error| io_error(source, error))?
                == fs::canonicalize(&target)
                    .await
                    .map_err(|error| io_error(&target, error))?
            {
                return Err(io_error(
                    &target,
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "destination is the source file",
                    ),
                ));
            }
            tracing::warn!(source = %source.display(), target = %target.display(), "replacing existing watch-folder file");
            fs::remove_file(&target)
                .await
                .map_err(|error| io_error(&target, error))?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(&target, error)),
    }
    fs::rename(source, &target)
        .await
        .map_err(|error| io_error(source, error))?;
    tracing::debug!(source = %source.display(), target = %target.display(), "moved watch-folder file");
    Ok(())
}

/// Scans one directory, importing stable files and moving each attempted import.
///
/// Only regular top-level files with `.torrent` or `.magnet` extensions are considered,
/// case-insensitively. Symlinks and subdirectories are not followed. An unchanged
/// 100 ms metadata observation precedes the exclusive lock so Windows writers can
/// finish during observation. Writers that pause longer than that window cannot
/// be detected reliably.
/// The importer supplies application defaults and returns `Ok(())` for duplicate adds.
/// Cancellation drops an in-flight importer and leaves its source in place. Once an
/// importer returns, its move is completed before cancellation is observed again.
///
/// # Errors
/// Returns directory enumeration errors or failures to quarantine a failed file. Read,
/// import and processed-move errors otherwise move the source to the error directory.
/// Already completed file operations are not rolled back.
pub async fn scan_once<F, Fut, E>(
    config: &WatchFolderConfig,
    cancel: &CancellationToken,
    importer: &mut F,
) -> Result<ScanReport, WatchFolderError>
where
    F: FnMut(WatchInput) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    let mut report = ScanReport::default();
    if config.path.as_os_str().is_empty() || cancel.is_cancelled() {
        return Ok(report);
    }
    let mut entries = fs::read_dir(&config.path)
        .await
        .map_err(|error| io_error(&config.path, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| io_error(&config.path, error))?
    {
        if cancel.is_cancelled() {
            break;
        }
        let path = entry.path();
        let Some(kind) = input_kind(&path) else {
            continue;
        };
        if !entry
            .file_type()
            .await
            .map_err(|error| io_error(&path, error))?
            .is_file()
        {
            continue;
        }
        let read = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            result = read_stable(&path, kind, cancel) => result,
        };
        let result = match read {
            Ok(Some(input)) => {
                let LockedInput { payload, lock } = input;
                let outcome = tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    result = importer(payload) => result,
                };
                // Closing before rename also permits moves on platforms that deny moving open files.
                drop(lock);
                match outcome {
                    Ok(()) => move_replacing(&path, &config.destination(true)).await,
                    Err(error) => {
                        tracing::warn!(path = %path.display(), %error, "watch-folder import failed");
                        move_replacing(&path, &config.destination(false)).await?;
                        report.failed += 1;
                        continue;
                    }
                }
            }
            Ok(None) => {
                report.skipped += 1;
                continue;
            }
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            tracing::warn!(path = %path.display(), %error, "watch-folder file processing failed");
            move_replacing(&path, &config.destination(false)).await?;
            report.failed += 1;
        } else {
            report.processed += 1;
        }
    }
    Ok(report)
}

fn next_scan(start: Instant, finished: Instant, interval: Duration, failures: u32) -> Instant {
    let regular = start + interval;
    if failures == 0 {
        regular
    } else {
        let backoff = MIN_INTERVAL
            .saturating_mul(1_u32 << failures.saturating_sub(1).min(5))
            .min(MAX_BACKOFF);
        regular.max(finished + backoff)
    }
}

/// Runs immediate-first scans until cancelled, refreshing configuration at each scan.
///
/// Scan starts are separated by at least `max(interval, 10 seconds)`; slow scans do
/// not overlap or cause catch-up bursts. Filesystem failures use capped exponential
/// backoff in addition to that cadence. A closed settings channel retains its last
/// configuration. Settings updates take effect on the next scheduled scan. The
/// caller owns task spawning and must finish startup before invoking this function.
pub async fn run<F, Fut, E>(
    config: watch::Receiver<WatchFolderConfig>,
    cancel: CancellationToken,
    mut importer: F,
) where
    F: FnMut(WatchInput) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    let mut failures = 0_u32;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let snapshot = config.borrow().clone();
        let start = Instant::now();
        match scan_once(&snapshot, &cancel, &mut importer).await {
            Ok(_) => failures = 0,
            Err(error) => {
                failures = failures.saturating_add(1);
                tracing::error!(%error, "watch-folder scan failed");
            }
        }
        let deadline = next_scan(start, Instant::now(), snapshot.scan_interval(), failures);
        tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            () = time::sleep_until(deadline) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::Notify;

    use super::*;

    fn config(directory: &TempDir) -> WatchFolderConfig {
        WatchFolderConfig {
            path: directory.path().to_path_buf(),
            interval: Duration::ZERO,
            ..WatchFolderConfig::default()
        }
    }

    async fn accept(_: WatchInput) -> Result<(), Infallible> {
        Ok(())
    }

    #[tokio::test]
    async fn imports_torrent_bytes_and_moves_to_processed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.torrent");
        let bytes = vec![0, 1, 255, 42];
        fs::write(&path, &bytes).await.unwrap();
        let mut received = Vec::new();
        let report = scan_once(
            &config(&directory),
            &CancellationToken::new(),
            &mut |input| {
                received.push(input);
                std::future::ready(Ok::<(), Infallible>(()))
            },
        )
        .await
        .unwrap();
        assert_eq!(received, vec![WatchInput::Torrent(bytes.clone())]);
        assert_eq!(report.processed, 1);
        assert!(!path.exists());
        assert_eq!(
            fs::read(directory.path().join("processed/test.torrent"))
                .await
                .unwrap(),
            bytes
        );
    }

    #[tokio::test]
    async fn imports_untrimmed_magnet_text_and_moves_to_processed() {
        let directory = tempfile::tempdir().unwrap();
        let text = " \r\nmagnet:?xt=urn:btih:123\r\n ";
        fs::write(directory.path().join("test.magnet"), text)
            .await
            .unwrap();
        let mut received = Vec::new();
        scan_once(
            &config(&directory),
            &CancellationToken::new(),
            &mut |input| {
                received.push(input);
                std::future::ready(Ok::<(), Infallible>(()))
            },
        )
        .await
        .unwrap();
        assert_eq!(received, vec![WatchInput::Magnet(text.into())]);
        assert!(!directory.path().join("test.magnet").exists());
        assert_eq!(
            fs::read_to_string(directory.path().join("processed/test.magnet"))
                .await
                .unwrap(),
            text
        );
    }

    #[tokio::test]
    async fn ignores_unrelated_files_and_never_scans_subfolders() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("test.txt"), "untouched")
            .await
            .unwrap();
        for child in ["processed", "error", "nested"] {
            fs::create_dir(directory.path().join(child)).await.unwrap();
            fs::write(directory.path().join(child).join("nested.torrent"), b"data")
                .await
                .unwrap();
        }
        let calls = AtomicUsize::new(0);
        let report = scan_once(&config(&directory), &CancellationToken::new(), &mut |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok::<(), Infallible>(()))
        })
        .await
        .unwrap();
        assert_eq!(report, ScanReport::default());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(directory.path().join("test.txt").exists());
        for child in ["processed", "error", "nested"] {
            assert!(directory.path().join(child).join("nested.torrent").exists());
        }
    }

    #[tokio::test]
    async fn extensions_are_case_insensitive_and_duplicate_adds_are_successful() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["one.TORRENT", "two.ToRrEnT", "three.MAGNET", "four.MaGnEt"] {
            fs::write(directory.path().join(name), b"data")
                .await
                .unwrap();
        }
        let report = scan_once(&config(&directory), &CancellationToken::new(), &mut accept)
            .await
            .unwrap();
        assert_eq!(report.processed, 4);
        assert_eq!(report.failed, 0);
    }

    #[tokio::test]
    async fn locked_file_is_deferred_then_imported_after_unlock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locked.torrent");
        fs::write(&path, b"data").await.unwrap();
        let lock = File::open(&path).unwrap();
        FileExt::lock_exclusive(&lock).unwrap();
        let report = scan_once(&config(&directory), &CancellationToken::new(), &mut accept)
            .await
            .unwrap();
        assert_eq!(report.skipped, 1);
        assert!(path.exists());
        assert!(!directory.path().join("error").exists());
        FileExt::unlock(&lock).unwrap();
        let report = scan_once(&config(&directory), &CancellationToken::new(), &mut accept)
            .await
            .unwrap();
        assert_eq!(report.processed, 1);
    }

    #[tokio::test]
    async fn changing_file_is_deferred_without_import_or_quarantine() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("writing.torrent");
        fs::write(&path, b"start").await.unwrap();
        let writer_cancel = CancellationToken::new();
        let writer_stop = writer_cancel.clone();
        let writer_path = path.clone();
        let writer = tokio::spawn(async move {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(writer_path)
                .await
                .unwrap();
            loop {
                tokio::select! {
                    () = writer_stop.cancelled() => break,
                    () = time::sleep(Duration::from_millis(5)) => {
                        file.write_all(b"more").await.unwrap();
                        file.flush().await.unwrap();
                    }
                }
            }
        });
        let report = scan_once(&config(&directory), &CancellationToken::new(), &mut accept)
            .await
            .unwrap();
        writer_cancel.cancel();
        writer.await.unwrap();
        assert_eq!(report.skipped, 1);
        assert!(path.exists());
        assert!(!directory.path().join("error").exists());
        let report = scan_once(&config(&directory), &CancellationToken::new(), &mut accept)
            .await
            .unwrap();
        assert_eq!(report.processed, 1);
    }

    #[tokio::test]
    async fn import_and_text_read_failures_move_to_error_and_continue() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("invalid.magnet"), [255])
            .await
            .unwrap();
        fs::write(directory.path().join("rejected.torrent"), b"reject")
            .await
            .unwrap();
        let calls = AtomicUsize::new(0);
        let report = scan_once(&config(&directory), &CancellationToken::new(), &mut |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Err::<(), _>(io::Error::other("import rejected")))
        })
        .await
        .unwrap();
        assert_eq!(report.failed, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(directory.path().join("error/invalid.magnet").exists());
        assert!(directory.path().join("error/rejected.torrent").exists());
    }

    #[tokio::test]
    async fn processed_move_failure_falls_back_to_error_directory() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("test.torrent"), b"data")
            .await
            .unwrap();
        fs::write(directory.path().join("processed"), b"not a directory")
            .await
            .unwrap();
        let report = scan_once(&config(&directory), &CancellationToken::new(), &mut accept)
            .await
            .unwrap();
        assert_eq!(report.failed, 1);
        assert!(directory.path().join("error/test.torrent").exists());
    }

    #[tokio::test]
    async fn failed_error_move_returns_error_and_preserves_source() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("test.torrent"), b"data")
            .await
            .unwrap();
        fs::write(directory.path().join("error"), b"not a directory")
            .await
            .unwrap();
        let result = scan_once(&config(&directory), &CancellationToken::new(), &mut |_| {
            std::future::ready(Err::<(), _>(io::Error::other("import rejected")))
        })
        .await;
        assert!(result.is_err());
        assert!(directory.path().join("test.torrent").exists());
    }

    #[tokio::test]
    async fn custom_destinations_replace_existing_files_on_both_paths() {
        let directory = tempfile::tempdir().unwrap();
        let processed = tempfile::tempdir().unwrap();
        let errors = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("good.torrent"), b"new good")
            .await
            .unwrap();
        fs::write(directory.path().join("bad.magnet"), b"new bad")
            .await
            .unwrap();
        fs::write(processed.path().join("good.torrent"), b"old good")
            .await
            .unwrap();
        fs::write(errors.path().join("bad.magnet"), b"old bad")
            .await
            .unwrap();
        let mut settings = config(&directory);
        settings.processed_path = Some(processed.path().to_path_buf());
        settings.error_path = Some(errors.path().to_path_buf());
        let report = scan_once(&settings, &CancellationToken::new(), &mut |input| {
            std::future::ready(match input {
                WatchInput::Torrent(_) => Ok(()),
                WatchInput::Magnet(_) => Err(io::Error::other("rejected")),
            })
        })
        .await
        .unwrap();
        assert_eq!((report.processed, report.failed), (1, 1));
        assert_eq!(
            fs::read(processed.path().join("good.torrent"))
                .await
                .unwrap(),
            b"new good"
        );
        assert_eq!(
            fs::read(errors.path().join("bad.magnet")).await.unwrap(),
            b"new bad"
        );
        assert!(!directory.path().join("processed").exists());
        assert!(!directory.path().join("error").exists());
    }

    #[tokio::test]
    async fn destination_equal_to_watch_folder_does_not_delete_source() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("test.torrent"), b"data")
            .await
            .unwrap();
        let mut settings = config(&directory);
        settings.processed_path = Some(directory.path().to_path_buf());
        let report = scan_once(&settings, &CancellationToken::new(), &mut accept)
            .await
            .unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(
            fs::read(directory.path().join("error/test.torrent"))
                .await
                .unwrap(),
            b"data"
        );
    }

    #[tokio::test]
    async fn starts_immediately_and_cancels_while_waiting_for_next_scan() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("test.torrent"), b"data")
            .await
            .unwrap();
        let (_sender, receiver) = watch::channel(config(&directory));
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let imported = Arc::new(Notify::new());
        let notification = imported.clone();
        let task = tokio::spawn(run(receiver, stop, move |_| {
            notification.notify_one();
            std::future::ready(Ok::<_, Infallible>(()))
        }));
        time::timeout(Duration::from_millis(500), imported.notified())
            .await
            .unwrap();
        cancel.cancel();
        time::timeout(Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap();
        assert!(directory.path().join("processed/test.torrent").exists());
    }

    #[tokio::test]
    async fn cancellation_drops_pending_import_and_keeps_source() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("test.torrent"), b"data")
            .await
            .unwrap();
        let settings = config(&directory);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let entered = Arc::new(Notify::new());
        let notification = entered.clone();
        let task = tokio::spawn(async move {
            scan_once(&settings, &stop, &mut |_| {
                notification.notify_one();
                std::future::pending::<Result<(), Infallible>>()
            })
            .await
        });
        time::timeout(Duration::from_millis(500), entered.notified())
            .await
            .unwrap();
        cancel.cancel();
        time::timeout(Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(directory.path().join("test.torrent").exists());
        assert!(!directory.path().join("processed").exists());
        assert!(!directory.path().join("error").exists());
    }

    #[tokio::test]
    async fn pre_cancelled_scan_and_disabled_watch_do_no_work() {
        let directory = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let missing = WatchFolderConfig {
            path: directory.path().join("missing"),
            ..WatchFolderConfig::default()
        };
        assert_eq!(
            scan_once(&missing, &cancel, &mut accept).await.unwrap(),
            ScanReport::default()
        );
        assert_eq!(
            scan_once(
                &WatchFolderConfig::default(),
                &CancellationToken::new(),
                &mut accept
            )
            .await
            .unwrap(),
            ScanReport::default()
        );
    }

    #[test]
    fn cadence_is_start_to_start_with_capped_failure_backoff() {
        let start = Instant::now();
        let finished = start + Duration::from_secs(3);
        assert_eq!(
            next_scan(start, finished, MIN_INTERVAL, 0),
            start + MIN_INTERVAL
        );
        assert_eq!(
            next_scan(start, finished, MIN_INTERVAL, 1),
            finished + MIN_INTERVAL
        );
        assert_eq!(
            next_scan(start, finished, MIN_INTERVAL, 2),
            finished + Duration::from_secs(20)
        );
        assert_eq!(
            next_scan(start, finished, MIN_INTERVAL, u32::MAX),
            finished + MAX_BACKOFF
        );
        let slow_finish = start + Duration::from_secs(40);
        assert!(next_scan(start, slow_finish, MIN_INTERVAL, 0) <= slow_finish);
        assert_eq!(
            config(&tempfile::tempdir().unwrap()).scan_interval(),
            MIN_INTERVAL
        );
        let settings = WatchFolderConfig {
            interval: Duration::from_secs(45),
            ..WatchFolderConfig::default()
        };
        assert_eq!(settings.scan_interval(), Duration::from_secs(45));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_inputs_are_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("source.torrent");
        fs::write(&target, b"data").await.unwrap();
        std::os::unix::fs::symlink(&target, directory.path().join("linked.torrent")).unwrap();
        let report = scan_once(&config(&directory), &CancellationToken::new(), &mut accept)
            .await
            .unwrap();
        assert_eq!(report, ScanReport::default());
        assert!(target.exists());
        assert!(directory.path().join("linked.torrent").exists());
    }
}

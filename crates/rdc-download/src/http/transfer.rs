//! Fetching the chunks of a file into its partial file.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Response, StatusCode};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::connection::{ByteRange, Context};
use super::error::TransferError;
use super::plan::{ChunkRange, plan_chunks};
use super::probe::RemoteFile;

/// Suffix of the file a download is written to until it completes.
const PARTIAL_SUFFIX: &str = ".download";

/// The file a download of `target` is written to until it completes: `<target>.download`.
pub(crate) fn partial_path(target: &Path) -> PathBuf {
    let mut name = OsString::from(target.as_os_str());
    name.push(PARTIAL_SUFFIX);
    PathBuf::from(name)
}

/// Removes `path` if it exists.
pub(crate) async fn remove_file_if_exists(path: &Path) -> Result<(), TransferError> {
    match tokio::fs::remove_file(path).await {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(TransferError::io(path, error))
        }
        _ => Ok(()),
    }
}

/// Downloads `remote` into `partial` and returns the final size in bytes.
pub(crate) async fn fetch_file(
    context: &Arc<Context>,
    remote: &RemoteFile,
    partial: &Path,
) -> Result<u64, TransferError> {
    let chunk_count = context.config().effective_chunk_count();
    let chunks = plan_chunks(remote.size, remote.supports_ranges, chunk_count);
    prepare_file(partial, remote.size).await?;
    tracing::debug!(
        "Downloading {} ({:?} bytes, ranges: {}) in {} chunk(s)",
        remote.url,
        remote.size,
        remote.supports_ranges,
        chunks.len()
    );
    fetch_chunks(context, remote, partial, chunks).await?;
    let received = context.received();
    if remote.size.is_none() {
        let file = OpenOptions::new()
            .write(true)
            .open(partial)
            .await
            .map_err(|error| TransferError::io(partial, error))?;
        file.set_len(received)
            .await
            .map_err(|error| TransferError::io(partial, error))?;
    }
    Ok(remote.size.unwrap_or(received))
}

/// Creates (or truncates) the partial file and reserves `size` bytes when known.
async fn prepare_file(partial: &Path, size: Option<u64>) -> Result<(), TransferError> {
    let file = File::create(partial)
        .await
        .map_err(|error| TransferError::io(partial, error))?;
    if let Some(size) = size {
        file.set_len(size)
            .await
            .map_err(|error| TransferError::io(partial, error))?;
    }
    Ok(())
}

/// Fetches `chunks` with bounded concurrency; the first failure stops the others.
async fn fetch_chunks(
    context: &Arc<Context>,
    remote: &RemoteFile,
    partial: &Path,
    chunks: Vec<ChunkRange>,
) -> Result<(), TransferError> {
    let attempt = context.cancel.child_token();
    let ranged = chunks.len() > 1;
    let timeout = context.config().read_timeout();
    let mut pending: VecDeque<Chunk> = chunks
        .into_iter()
        .map(|range| Chunk {
            range,
            position: 0,
            timeout,
            ranged,
        })
        .collect();
    let mut running = JoinSet::new();
    let mut config = context.config.clone();
    let mut config_open = true;
    let mut failure = None;
    loop {
        if failure.is_none() {
            let limit = config.borrow().effective_parallel_connections();
            while running.len() < limit
                && let Some(chunk) = pending.pop_front()
            {
                let worker = ChunkWorker {
                    context: Arc::clone(context),
                    remote: remote.clone(),
                    partial: partial.to_owned(),
                    cancel: attempt.clone(),
                };
                running.spawn(worker.run(chunk));
            }
        }
        if running.is_empty() {
            break;
        }
        tokio::select! {
            Some(joined) = running.join_next() => {
                let result = joined.unwrap_or_else(|error| Err(TransferError::Interrupted(error.to_string())));
                if let Err(error) = result
                    && failure.is_none()
                {
                    failure = Some(error);
                    attempt.cancel();
                }
            }
            changed = config.changed(), if config_open => config_open = changed.is_ok(),
        }
    }
    failure.map_or(Ok(()), Err)
}

/// Progress of one chunk.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    range: ChunkRange,
    /// Bytes of the range written so far.
    position: u64,
    /// Current per-read timeout.
    timeout: Duration,
    /// Whether the chunk is one of several, so even its first request names its range.
    ranged: bool,
}

impl Chunk {
    fn remaining(self) -> Option<u64> {
        self.range
            .len()
            .map(|len| len.saturating_sub(self.position))
    }
}

struct ChunkWorker {
    context: Arc<Context>,
    remote: RemoteFile,
    partial: PathBuf,
    cancel: CancellationToken,
}

impl ChunkWorker {
    /// Fetches the chunk; a stream ending before the range is complete is an error.
    async fn run(self, mut chunk: Chunk) -> Result<(), TransferError> {
        self.fetch(&mut chunk).await?;
        match chunk.remaining() {
            Some(remaining) if remaining > 0 => Err(TransferError::Incomplete {
                received: chunk.position,
                expected: chunk.position + remaining,
            }),
            _ => Ok(()),
        }
    }

    /// Sends one request for the rest of the chunk and writes what arrives.
    async fn fetch(&self, chunk: &mut Chunk) -> Result<(), TransferError> {
        let offset = chunk.range.start + chunk.position;
        // A chunk names its range when it is one of several or continues a previous request
        // (open-ended when the size is unknown).
        let range = (chunk.ranged || chunk.position > 0).then_some(ByteRange {
            start: offset,
            end: chunk.range.end,
        });
        let mut response = self
            .context
            .get(&self.remote.url, range, &self.cancel)
            .await?;
        if range.is_some() && offset > 0 && response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(TransferError::RangeIgnored);
        }
        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.partial)
            .await
            .map_err(|error| self.io_error(error))?;
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|error| self.io_error(error))?;
        let mut writer =
            BufWriter::with_capacity(self.context.config().effective_buffer_size(), file);
        let result = self.copy_body(&mut response, &mut writer, chunk).await;
        // Bytes counted in `chunk.position` must reach the file before anything else happens.
        writer.flush().await.map_err(|error| self.io_error(error))?;
        result
    }

    async fn copy_body(
        &self,
        response: &mut Response,
        writer: &mut BufWriter<File>,
        chunk: &mut Chunk,
    ) -> Result<(), TransferError> {
        loop {
            let remaining = chunk.remaining();
            if remaining == Some(0) {
                return Ok(());
            }
            let read = tokio::time::timeout(chunk.timeout, response.chunk());
            let bytes = match Context::cancellable(&self.cancel, read).await? {
                Err(_) => return Err(TransferError::ReadTimeout(chunk.timeout)),
                Ok(Err(error)) => return Err(TransferError::Http(error)),
                Ok(Ok(None)) => return Ok(()),
                Ok(Ok(Some(bytes))) => bytes,
            };
            let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            let take = remaining.map_or(length, |remaining| remaining.min(length));
            let end = usize::try_from(take).unwrap_or(bytes.len());
            writer
                .write_all(&bytes[..end])
                .await
                .map_err(|error| self.io_error(error))?;
            chunk.position += take;
            self.context.add_received(take);
        }
    }

    fn io_error(&self, error: io::Error) -> TransferError {
        TransferError::io(&self.partial, error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_appends_download_suffix() {
        assert_eq!(
            partial_path(Path::new("/data/Movie/file.mkv")),
            PathBuf::from("/data/Movie/file.mkv.download")
        );
        assert_eq!(
            partial_path(Path::new("noext")),
            PathBuf::from("noext.download")
        );
    }

    #[tokio::test]
    async fn removing_a_missing_file_succeeds() {
        let dir = tempfile::tempdir().expect("temp dir");
        remove_file_if_exists(&dir.path().join("absent"))
            .await
            .expect("no error");
    }
}

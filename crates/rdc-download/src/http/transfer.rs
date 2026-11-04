//! Fetching a file into its partial file.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};

use super::connection::Context;
use super::error::TransferError;
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
    let io_error = |error| TransferError::io(partial, error);
    let mut response = context.get(&remote.url, None, &context.cancel).await?;
    let file = File::create(partial).await.map_err(io_error)?;
    let config = context.config();
    let timeout = config.read_timeout();
    let mut writer = BufWriter::with_capacity(config.effective_buffer_size(), file);
    loop {
        let read = tokio::time::timeout(timeout, response.chunk());
        let bytes = match Context::cancellable(&context.cancel, read).await? {
            Err(_) => return Err(TransferError::ReadTimeout(timeout)),
            Ok(Ok(Some(bytes))) => bytes,
            Ok(Ok(None)) => break,
            Ok(Err(error)) => return Err(TransferError::Http(error)),
        };
        writer.write_all(&bytes).await.map_err(io_error)?;
        context.add_received(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    }
    writer.flush().await.map_err(io_error)?;
    Ok(context.received())
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

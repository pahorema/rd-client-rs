//! Fetching a file.

use std::path::Path;
use std::sync::Arc;

use reqwest::Url;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};

use super::connection::Context;
use super::error::TransferError;

/// Downloads `url` into `path` and returns the final size in bytes.
pub(crate) async fn fetch_file(
    context: &Arc<Context>,
    url: &Url,
    path: &Path,
) -> Result<u64, TransferError> {
    let io_error = |error| TransferError::io(path, error);
    let mut response = context.get(url, &context.cancel).await?;
    let file = File::create(path).await.map_err(io_error)?;
    let mut writer = BufWriter::with_capacity(context.config().effective_buffer_size(), file);
    loop {
        let bytes = match Context::cancellable(&context.cancel, response.chunk()).await? {
            Ok(Some(bytes)) => bytes,
            Ok(None) => break,
            Err(error) => return Err(TransferError::Http(error)),
        };
        writer.write_all(&bytes).await.map_err(io_error)?;
        context.add_received(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    }
    writer.flush().await.map_err(io_error)?;
    Ok(context.received())
}

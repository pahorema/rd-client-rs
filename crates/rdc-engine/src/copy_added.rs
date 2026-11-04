//! Writes optional torrent and magnet copies without interrupting torrent addition.

use std::path::{Path, PathBuf};

use rdc_model::paths::remove_invalid_file_name_chars;
use tokio::fs;

/// The already-decoded source payload to preserve in the configured copy directory.
#[derive(Clone, Copy, Debug)]
pub enum AddedPayload<'a> {
    /// Raw torrent metadata bytes.
    Torrent(&'a [u8]),
    /// A complete magnet link, preserved verbatim.
    Magnet(&'a str),
}

/// Writes a sanitized, replaceable copy, returning its path when successful.
///
/// Missing/blank directory, name, or payload disables the copy. Filesystem failures
/// are logged and return `None`, so callers never need to fail torrent addition.
/// A matching filename extension is preserved case-insensitively.
pub async fn copy_added(
    directory: Option<&Path>,
    name: Option<&str>,
    payload: AddedPayload<'_>,
) -> Option<PathBuf> {
    let directory = directory.filter(|path| {
        path.as_os_str()
            .to_str()
            .is_none_or(|value| !value.trim().is_empty())
    })?;
    let name = name.filter(|name| !name.trim().is_empty())?;
    let (extension, bytes) = match payload {
        AddedPayload::Torrent(bytes) if !bytes.is_empty() => (".torrent", bytes),
        AddedPayload::Magnet(text) if !text.trim().is_empty() => (".magnet", text.as_bytes()),
        AddedPayload::Torrent(_) | AddedPayload::Magnet(_) => return None,
    };
    let mut name = remove_invalid_file_name_chars(name);
    if !name.to_ascii_lowercase().ends_with(extension) {
        name.push_str(extension);
    }
    let destination = directory.join(name);
    let result = async {
        fs::create_dir_all(directory).await?;
        match fs::remove_file(&destination).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::write(&destination, bytes).await
    }
    .await;
    match result {
        Ok(()) => Some(destination),
        Err(error) => {
            tracing::error!(path = %directory.display(), %error, "Unable to create torrent copy");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_directory_and_sanitizes_torrent_name() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("nested/copies");
        let path = copy_added(
            Some(&directory),
            Some("test/../file"),
            AddedPayload::Torrent(b"metadata"),
        )
        .await
        .unwrap();
        assert_eq!(path, directory.join("testfile.torrent"));
        assert_eq!(fs::read(path).await.unwrap(), b"metadata");
    }

    #[tokio::test]
    async fn preserves_magnet_text_and_existing_extension_and_replaces() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("name.MAGNET");
        fs::write(&path, "old").await.unwrap();
        let text = " magnet:?xt=urn:btih:123\r\n";
        assert_eq!(
            copy_added(
                Some(temp.path()),
                Some("name.MAGNET"),
                AddedPayload::Magnet(text)
            )
            .await,
            Some(path.clone())
        );
        assert_eq!(fs::read_to_string(path).await.unwrap(), text);
    }

    #[tokio::test]
    async fn disabled_or_missing_payload_does_not_write() {
        let temp = tempfile::tempdir().unwrap();
        for (directory, name, payload) in [
            (None, Some("name"), AddedPayload::Torrent(b"data")),
            (Some(temp.path()), None, AddedPayload::Magnet("magnet:?")),
            (
                Some(temp.path()),
                Some("  "),
                AddedPayload::Magnet("magnet:?"),
            ),
            (Some(temp.path()), Some("name"), AddedPayload::Torrent(b"")),
            (Some(temp.path()), Some("name"), AddedPayload::Magnet(" \n")),
        ] {
            assert!(copy_added(directory, name, payload).await.is_none());
        }
        assert!(
            fs::read_dir(temp.path())
                .await
                .unwrap()
                .next_entry()
                .await
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn replaces_symlink_without_modifying_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        fs::write(&original, "untouched").await.unwrap();
        std::os::unix::fs::symlink(&original, temp.path().join("name.torrent")).unwrap();
        let path = copy_added(
            Some(temp.path()),
            Some("name"),
            AddedPayload::Torrent(b"new"),
        )
        .await
        .unwrap();
        assert!(!fs::symlink_metadata(&path).await.unwrap().is_symlink());
        assert_eq!(fs::read(path).await.unwrap(), b"new");
        assert_eq!(fs::read_to_string(original).await.unwrap(), "untouched");
    }

    #[tokio::test]
    async fn filesystem_errors_do_not_escape() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, "not a directory").await.unwrap();
        assert!(
            copy_added(Some(&file), Some("name"), AddedPayload::Torrent(b"data"))
                .await
                .is_none()
        );
    }
}

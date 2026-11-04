//! Behaviour of the symlink engine against temporary mount directories.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use rdc_download::{CANCELLED_MESSAGE, DownloadEngine, SymlinkDownload, SymlinkRequest};

struct Layout {
    _root: tempfile::TempDir,
    mount: PathBuf,
    downloads: PathBuf,
}

impl Layout {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temp dir");
        let mount = root.path().join("mount");
        let downloads = root.path().join("downloads");
        std::fs::create_dir_all(&mount).expect("mount");
        std::fs::create_dir_all(&downloads).expect("downloads");
        Self {
            _root: root,
            mount,
            downloads,
        }
    }

    /// Creates a file below the mount and returns its path.
    fn mount_file(&self, relative: &str) -> PathBuf {
        let path = self.mount.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(&path, relative.as_bytes()).expect("file");
        path
    }

    fn request(&self, relative: &str) -> SymlinkRequest {
        SymlinkRequest {
            mount_path: format!("{}/", self.mount.display()),
            relative_path: relative.to_owned(),
            destination: self.downloads.join(file_name(relative)),
        }
    }
}

fn file_name(relative: &str) -> &str {
    relative.rsplit('/').next().expect("file name")
}

fn engine(request: SymlinkRequest) -> SymlinkDownload {
    SymlinkDownload::new(request)
}

async fn link(request: SymlinkRequest) -> (Result<String, String>, PathBuf) {
    let destination = request.destination.clone();
    let engine = engine(request);
    let result = engine
        .start()
        .await
        .map_err(|error| error.message().to_owned());
    assert_eq!(
        engine.completion().outcome(),
        Some(result.clone().map(|_| ())),
        "completion matches start"
    );
    (result, destination)
}

fn assert_links_to(link: &Path, source: &Path) {
    assert_eq!(std::fs::read_link(link).expect("is a link"), source);
    assert_eq!(
        std::fs::read(link).expect("link resolves"),
        std::fs::read(source).expect("source")
    );
}

#[tokio::test]
async fn links_the_file_found_at_the_expected_path() {
    let layout = Layout::new();
    let source = layout.mount_file("Movie/Movie.mkv");
    let (result, destination) = link(layout.request("Movie/Movie.mkv")).await;
    assert_eq!(result, Ok(source.display().to_string()));
    assert_links_to(&destination, &source);
}

#[tokio::test]
async fn files_without_extension_are_linked() {
    let layout = Layout::new();
    let source = layout.mount_file("Show.S01/S01");
    let (result, destination) = link(layout.request("Show.S01/S01")).await;
    assert_eq!(result, Ok(source.display().to_string()));
    assert_links_to(&destination, &source);
}

#[tokio::test]
async fn missing_mount_is_reported() {
    let layout = Layout::new();
    let mut request = layout.request("Movie/Movie.mkv");
    request.mount_path = format!("{}/absent/", layout.mount.display());
    let (result, _) = link(request).await;
    assert_eq!(
        result,
        Err(format!(
            "Mount path {}/absent does not exist!",
            layout.mount.display()
        ))
    );
}

#[tokio::test]
async fn missing_files_are_not_found() {
    let layout = Layout::new();
    layout.mount_file("torrents/Movie/Movie.mkv");
    let (result, destination) = link(layout.request("Movie/Movie.mkv")).await;
    assert_eq!(
        result,
        Err("Could not find file from rclone mount!".to_owned())
    );
    assert!(std::fs::symlink_metadata(destination).is_err());
}

#[tokio::test]
async fn cancel_before_start_completes_the_engine() {
    let layout = Layout::new();
    let engine = engine(layout.request("Movie/Movie.mkv"));
    engine.cancel().await;
    assert_eq!(
        engine.completion().outcome(),
        Some(Err(CANCELLED_MESSAGE.to_owned()))
    );
    let error = engine.start().await.expect_err("cancelled");
    assert_eq!(error.message(), CANCELLED_MESSAGE);
}

#[tokio::test]
async fn compressed_files_are_refused() {
    let layout = Layout::new();
    for name in [
        "Movie/archive.zip",
        "Movie/archive.rar",
        "Movie/archive.tar",
    ] {
        layout.mount_file(name);
        let (result, _) = link(layout.request(name)).await;
        assert_eq!(
            result,
            Err("Cant handle compressed files with symlink downloader".to_owned()),
            "file {name}"
        );
    }
}

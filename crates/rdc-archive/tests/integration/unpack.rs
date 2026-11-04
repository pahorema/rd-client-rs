use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rdc_archive::{Error, UnpackRequest, unpack};
use tokio_util::sync::CancellationToken;

use crate::common::{big_text, copy_fixtures, files_below, fixtures, write_zip};

const MULTI: [&str; 3] = ["multi.part1.rar", "multi.part2.rar", "multi.part3.rar"];

/// A download directory with the torrent folder `Show` already created.
struct Downloads {
    _dir: tempfile::TempDir,
    destination: PathBuf,
    torrent: PathBuf,
}

impl Downloads {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary directory can be created");
        let destination = dir.path().join("downloads");
        let torrent = destination.join("Show");
        fs::create_dir_all(&torrent).expect("torrent folder can be created");
        Self {
            _dir: dir,
            destination,
            torrent,
        }
    }

    fn request(&self, archive: &Path) -> UnpackRequest {
        UnpackRequest {
            archive: archive.to_path_buf(),
            destination: self.destination.clone(),
            torrent_name: "Show".to_owned(),
        }
    }
}

async fn run(request: UnpackRequest) -> Result<(), Error> {
    unpack(request, |_| {}, CancellationToken::new()).await
}

#[tokio::test]
async fn a_missing_archive_is_not_an_error() {
    let downloads = Downloads::new();

    run(downloads.request(&downloads.torrent.join("gone.rar")))
        .await
        .unwrap();

    assert_eq!(files_below(&downloads.destination), Vec::<String>::new());
}

#[tokio::test]
async fn content_without_a_root_folder_goes_into_the_torrent_folder() {
    let downloads = Downloads::new();
    let archive = downloads.torrent.join("episode.zip");
    write_zip(
        &archive,
        &[("episode.mkv", b"video"), ("subs/en.srt", b"text")],
    );

    run(downloads.request(&archive)).await.unwrap();

    assert_eq!(
        files_below(&downloads.destination),
        ["Show/episode.mkv", "Show/subs/en.srt"]
    );
}

#[tokio::test]
async fn content_with_a_root_folder_named_like_the_torrent_is_extracted_in_place() {
    let downloads = Downloads::new();
    let archive = downloads.torrent.join("show.zip");
    write_zip(&archive, &[("Show/episode.mkv", b"video")]);

    run(downloads.request(&archive)).await.unwrap();

    assert_eq!(files_below(&downloads.destination), ["Show/episode.mkv"]);
}

#[tokio::test]
async fn a_backslash_root_folder_is_recognised() {
    let downloads = Downloads::new();
    let archive = downloads.torrent.join("show.zip");
    write_zip(&archive, &[("Show\\episode.mkv", b"video")]);

    run(downloads.request(&archive)).await.unwrap();

    assert_eq!(files_below(&downloads.destination), ["Show/episode.mkv"]);
}

#[tokio::test]
async fn a_root_folder_only_starting_with_the_torrent_name_is_not_the_torrent_folder() {
    let downloads = Downloads::new();
    let archive = downloads.torrent.join("show.zip");
    write_zip(&archive, &[("Show.Extras/clip.mkv", b"video")]);

    run(downloads.request(&archive)).await.unwrap();

    assert_eq!(
        files_below(&downloads.destination),
        ["Show/Show.Extras/clip.mkv"]
    );
}

#[tokio::test]
async fn the_archive_is_deleted_after_a_successful_unpack() {
    let downloads = Downloads::new();
    let archive = copy_fixtures(&downloads.torrent, &["nested.rar"]);

    run(downloads.request(&archive)).await.unwrap();

    assert!(!archive.exists());
    assert_eq!(
        files_below(&downloads.torrent),
        ["folder/a.txt", "folder/sub/b.txt", "top.txt"]
    );
}

#[tokio::test]
async fn every_volume_of_an_unpacked_set_is_deleted() {
    let downloads = Downloads::new();
    let archive = copy_fixtures(&downloads.torrent, &MULTI);

    run(downloads.request(&archive)).await.unwrap();

    assert_eq!(files_below(&downloads.torrent), ["big.txt"]);
    assert_eq!(
        fs::read_to_string(downloads.torrent.join("big.txt")).unwrap(),
        big_text()
    );
}

#[tokio::test]
async fn a_later_volume_is_left_for_its_first_volume() {
    let downloads = Downloads::new();
    copy_fixtures(&downloads.torrent, &MULTI);

    run(downloads.request(&downloads.torrent.join(MULTI[1])))
        .await
        .unwrap();

    assert_eq!(files_below(&downloads.torrent), MULTI);
}

#[tokio::test]
async fn a_failed_extraction_keeps_the_archive() {
    let downloads = Downloads::new();
    let archive = copy_fixtures(&downloads.torrent, &MULTI[..2]);

    let error = run(downloads.request(&archive)).await.unwrap_err();

    assert!(matches!(error, Error::Read { .. }), "{error}");
    assert!(archive.exists());
}

#[tokio::test]
async fn an_archive_wrapping_an_old_style_set_is_unpacked_through_the_set() {
    let downloads = Downloads::new();
    let archive = downloads.torrent.join("wrapper.zip");
    let volumes: Vec<(&str, Vec<u8>)> = ["legacy.rar", "legacy.r00", "legacy.r01"]
        .into_iter()
        .map(|name| (name, fs::read(fixtures().join(name)).unwrap()))
        .collect();
    let entries: Vec<(&str, &[u8])> = volumes
        .iter()
        .map(|(name, content)| (*name, content.as_slice()))
        .collect();
    write_zip(&archive, &entries);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);

    unpack(
        downloads.request(&archive),
        move |percent| record.lock().unwrap().push(percent),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(files_below(&downloads.destination), ["Show/big.txt"]);
    assert_eq!(
        fs::read_to_string(downloads.torrent.join("big.txt")).unwrap(),
        big_text()
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.last(), Some(&100));
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]), "{seen:?}");
}

#[tokio::test]
async fn a_wrapper_without_a_top_level_set_keeps_its_extracted_content() {
    let downloads = Downloads::new();
    let archive = downloads.torrent.join("wrapper.zip");
    write_zip(&archive, &[("sub/inner.r00", b"volume")]);

    run(downloads.request(&archive)).await.unwrap();

    let files = files_below(&downloads.torrent);
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].ends_with("/sub/inner.r00"), "{files:?}");
    assert!(!archive.exists());
}

#[tokio::test]
async fn progress_of_an_unpack_reaches_one_hundred() {
    let downloads = Downloads::new();
    let archive = copy_fixtures(&downloads.torrent, &["nested4.rar"]);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);

    unpack(
        downloads.request(&archive),
        move |percent| record.lock().unwrap().push(percent),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.first(), Some(&0));
    assert_eq!(seen.last(), Some(&100));
}

#[tokio::test]
async fn a_cancelled_unpack_keeps_the_archive() {
    let downloads = Downloads::new();
    let archive = copy_fixtures(&downloads.torrent, &["nested.rar"]);
    let cancel = CancellationToken::new();
    cancel.cancel();

    let error = unpack(downloads.request(&archive), |_| {}, cancel)
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Cancelled), "{error}");
    assert!(archive.exists());
    assert_eq!(files_below(&downloads.torrent), ["nested.rar"]);
}

#[tokio::test]
async fn a_torrent_name_leaving_the_download_directory_is_refused() {
    let downloads = Downloads::new();
    let archive = downloads.torrent.join("episode.zip");
    write_zip(&archive, &[("episode.mkv", b"video")]);
    let request = UnpackRequest {
        torrent_name: "../elsewhere".to_owned(),
        ..downloads.request(&archive)
    };

    let error = run(request).await.unwrap_err();

    assert!(matches!(error, Error::UnsafeName { .. }), "{error}");
    assert!(archive.exists());
}

#[test]
fn failure_messages_name_the_link_and_the_torrent() {
    let message = Error::Cancelled.unpack_failure_message(Some("https://host/file.rar"), "Show");

    assert_eq!(
        message,
        "An unexpected error occurred unpacking https://host/file.rar for torrent Show: \
         the unpack was cancelled"
    );
}

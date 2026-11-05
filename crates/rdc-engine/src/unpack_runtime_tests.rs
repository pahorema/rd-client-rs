use super::*;
use crate::{TorrentTemplate, testing::TestEngine};
use rdc_model::{DownloadInfo, TorrentFinishedAction};
use std::io::{Cursor, Write};
use tokio::fs;

const MAGNET: &str = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=Example";

fn zip_bytes(name: &str, content: &[u8]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file(name, zip::write::SimpleFileOptions::default())
        .expect("entry");
    archive.write_all(content).expect("content");
    archive.finish().expect("zip").into_inner()
}

async fn fixture(names: &[&str]) -> (TestEngine, Torrent) {
    let test = TestEngine::new().await.expect("engine");
    let torrent = test
        .engine
        .add_magnet(
            MAGNET,
            TorrentTemplate {
                finished_action: TorrentFinishedAction::None,
                ..TorrentTemplate::default()
            },
        )
        .await
        .expect("torrent");
    for name in names {
        test.store
            .downloads()
            .try_add(
                torrent.torrent_id,
                &DownloadInfo {
                    file_name: Some((*name).to_owned()),
                    restricted_link: format!("https://example.invalid/{name}"),
                },
            )
            .await
            .expect("download");
    }
    let torrent = test
        .store
        .torrents()
        .get(torrent.torrent_id)
        .await
        .expect("read")
        .expect("torrent");
    (test, torrent)
}

async fn downloaded(test: &TestEngine, download: &Download) {
    let now = Some(Timestamp::now());
    test.store
        .downloads()
        .update_link(download.download_id, &download.path)
        .await
        .expect("link");
    test.store
        .downloads()
        .update_download_started(download.download_id, now)
        .await
        .expect("started");
    test.store
        .downloads()
        .update_download_finished(download.download_id, now)
        .await
        .expect("finished");
    test.store
        .downloads()
        .update_unpacking_queued(download.download_id, now)
        .await
        .expect("unpack queued");
}

async fn current(test: &TestEngine, id: uuid::Uuid) -> Torrent {
    test.store
        .torrents()
        .get(id)
        .await
        .expect("read")
        .expect("torrent")
}

async fn wait_for_unpacks(test: &TestEngine) {
    let jobs: Vec<_> = test
        .engine
        .inner
        .registry
        .unpacks
        .lock()
        .expect("registry")
        .values()
        .cloned()
        .collect();
    for job in jobs {
        let mut completion = job.completion.clone();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), completion.wait())
            .await
            .expect("unpack timeout");
    }
}

#[tokio::test]
async fn zip_extraction_reports_percentage_and_marks_all_completion_timestamps() {
    let (test, torrent) = fixture(&["archive.zip"]).await;
    downloaded(&test, &torrent.downloads[0]).await;
    let torrent = current(&test, torrent.torrent_id).await;
    let (_, archive, _) = test
        .engine
        .output_paths(&torrent, &torrent.downloads[0])
        .await
        .expect("path");
    fs::write(&archive, zip_bytes("content.txt", b"expanded"))
        .await
        .expect("archive");
    test.engine.start_unpacks(&torrent, 1).await.expect("start");
    let active = test
        .engine
        .inner
        .registry
        .unpacks
        .lock()
        .expect("registry")
        .values()
        .next()
        .expect("active")
        .clone();
    wait_for_unpacks(&test).await;
    assert_eq!(*active.progress.borrow(), 100);
    test.engine
        .collect_completed_unpacks()
        .await
        .expect("collect");
    let torrent = current(&test, torrent.torrent_id).await;
    let download = &torrent.downloads[0];
    assert!(download.unpacking_started.is_some());
    assert!(download.unpacking_finished.is_some());
    assert!(download.completed.is_some());
    assert!(download.error.is_none());
    assert!(!archive.exists());
    assert_eq!(
        fs::read(archive.parent().expect("parent").join("content.txt"))
            .await
            .expect("expanded"),
        b"expanded"
    );
    test.engine.shutdown().await;
}

#[tokio::test]
async fn multi_volume_sets_wait_for_every_volume_and_share_one_extraction_result() {
    let (test, torrent) = fixture(&["archive.part1.rar", "archive.part2.rar"]).await;
    downloaded(&test, &torrent.downloads[0]).await;
    let torrent = current(&test, torrent.torrent_id).await;
    test.engine
        .start_unpacks(&torrent, 1)
        .await
        .expect("held back");
    assert!(
        test.engine
            .inner
            .registry
            .unpacks
            .lock()
            .expect("registry")
            .is_empty()
    );
    assert!(
        current(&test, torrent.torrent_id).await.downloads[0]
            .unpacking_started
            .is_none()
    );
    downloaded(&test, &torrent.downloads[1]).await;
    let torrent = current(&test, torrent.torrent_id).await;
    for download in &torrent.downloads {
        let (_, path, _) = test
            .engine
            .output_paths(&torrent, download)
            .await
            .expect("path");
        fs::write(path, b"invalid rar").await.expect("volume");
    }
    test.engine
        .start_unpacks(&torrent, 1)
        .await
        .expect("one set");
    {
        let entries = test.engine.inner.registry.unpacks.lock().expect("registry");
        assert_eq!(entries.len(), 2);
        let mut entries = entries.values();
        assert!(Arc::ptr_eq(
            entries.next().expect("first"),
            entries.next().expect("second")
        ));
    }
    wait_for_unpacks(&test).await;
    test.engine
        .collect_completed_unpacks()
        .await
        .expect("collect");
    for download in current(&test, torrent.torrent_id).await.downloads {
        assert!(download.completed.is_some());
        assert!(download.error.is_some());
        assert!(download.unpacking_finished.is_none());
    }
    test.engine.shutdown().await;
}

#[tokio::test]
async fn a_failed_volume_finishes_queued_siblings_instead_of_waiting_forever() {
    let (test, torrent) = fixture(&["archive.rar", "archive.r00"]).await;
    downloaded(&test, &torrent.downloads[0]).await;
    test.store
        .downloads()
        .update_error(torrent.downloads[1].download_id, Some("transfer failed"))
        .await
        .expect("error");
    test.store
        .downloads()
        .update_completed(torrent.downloads[1].download_id, Some(Timestamp::now()))
        .await
        .expect("complete");
    let torrent = current(&test, torrent.torrent_id).await;
    test.engine
        .start_unpacks(&torrent, 1)
        .await
        .expect("failed set");
    let first = current(&test, torrent.torrent_id).await.downloads.remove(0);
    assert!(first.completed.is_some());
    assert!(first.error.expect("error").contains("archive volume"));
    test.engine.shutdown().await;
}

#[tokio::test]
async fn disabled_unpacking_nonarchives_and_symlinks_complete_without_extraction() {
    for (name, client, limit) in [
        ("file.txt", DownloadClient::Internal, 1),
        ("archive.zip", DownloadClient::Internal, 0),
        ("archive.zip", DownloadClient::Symlink, 1),
        ("archive.ZIP", DownloadClient::Internal, 1),
    ] {
        let (test, torrent) = fixture(&[name]).await;
        downloaded(&test, &torrent.downloads[0]).await;
        let mut torrent = current(&test, torrent.torrent_id).await;
        torrent.download_client = client;
        test.engine
            .start_unpacks(&torrent, limit)
            .await
            .expect("skip");
        let download = current(&test, torrent.torrent_id).await.downloads.remove(0);
        assert!(download.completed.is_some());
        assert!(download.unpacking_finished.is_some());
        assert!(
            test.engine
                .inner
                .registry
                .unpacks
                .lock()
                .expect("registry")
                .is_empty()
        );
        test.engine.shutdown().await;
    }
}

#[tokio::test]
async fn the_unpack_limit_counts_jobs_not_volume_rows() {
    let (test, torrent) = fixture(&["a.zip", "b.zip"]).await;
    for download in &torrent.downloads {
        downloaded(&test, download).await;
    }
    let torrent = current(&test, torrent.torrent_id).await;
    for download in &torrent.downloads {
        let (_, path, _) = test
            .engine
            .output_paths(&torrent, download)
            .await
            .expect("path");
        fs::write(path, zip_bytes("content.txt", b"data"))
            .await
            .expect("archive");
    }
    test.engine.start_unpacks(&torrent, 1).await.expect("first");
    assert_eq!(
        test.engine
            .inner
            .registry
            .unpacks
            .lock()
            .expect("registry")
            .len(),
        1
    );
    wait_for_unpacks(&test).await;
    test.engine
        .collect_completed_unpacks()
        .await
        .expect("collect first");
    let torrent = current(&test, torrent.torrent_id).await;
    test.engine
        .start_unpacks(&torrent, 1)
        .await
        .expect("second");
    wait_for_unpacks(&test).await;
    test.engine
        .collect_completed_unpacks()
        .await
        .expect("collect second");
    assert!(
        current(&test, torrent.torrent_id)
            .await
            .downloads
            .iter()
            .all(|download| download.completed.is_some())
    );
    test.engine.shutdown().await;
}

#[tokio::test]
async fn torbox_archive_hash_folder_is_flattened_by_the_unpack_procedure() {
    let (test, torrent) = fixture(&["archive.zip"]).await;
    downloaded(&test, &torrent.downloads[0]).await;
    let mut torrent = current(&test, torrent.torrent_id).await;
    torrent.client_kind = Some(Provider::TorBox);
    let (_, archive, _) = test
        .engine
        .output_paths(&torrent, &torrent.downloads[0])
        .await
        .expect("path");
    fs::write(
        &archive,
        zip_bytes(&format!("{}/inner/content.txt", torrent.hash), b"flat"),
    )
    .await
    .expect("archive");
    test.engine.start_unpacks(&torrent, 1).await.expect("start");
    wait_for_unpacks(&test).await;
    test.engine
        .collect_completed_unpacks()
        .await
        .expect("collect");
    assert_eq!(
        fs::read(archive.parent().expect("folder").join("content.txt"))
            .await
            .expect("flattened"),
        b"flat"
    );
    assert!(
        !archive
            .parent()
            .expect("folder")
            .join(&torrent.hash)
            .exists()
    );
    test.engine.shutdown().await;
}

#[tokio::test]
async fn escaping_zip_entries_fail_without_writing_outside_the_destination() {
    let (test, torrent) = fixture(&["archive.zip"]).await;
    downloaded(&test, &torrent.downloads[0]).await;
    let torrent = current(&test, torrent.torrent_id).await;
    let (_, archive, _) = test
        .engine
        .output_paths(&torrent, &torrent.downloads[0])
        .await
        .expect("path");
    fs::write(&archive, zip_bytes("../escaped.txt", b"unsafe"))
        .await
        .expect("archive");
    test.engine.start_unpacks(&torrent, 1).await.expect("start");
    wait_for_unpacks(&test).await;
    test.engine
        .collect_completed_unpacks()
        .await
        .expect("collect");
    assert!(
        current(&test, torrent.torrent_id).await.downloads[0]
            .error
            .is_some()
    );
    assert!(
        !Path::new(&test.engine.settings().download_client.download_path)
            .join("escaped.txt")
            .exists()
    );
    test.engine.shutdown().await;
}

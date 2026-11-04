use jiff::Timestamp;
use rdc_model::{Download, DownloadInfo};
use rdc_store::{AddResult, StoreError};
use uuid::Uuid;

use crate::common::TestStore;

fn info(link: &str) -> DownloadInfo {
    DownloadInfo {
        file_name: Some("file.mkv".to_owned()),
        restricted_link: link.to_owned(),
    }
}

fn instant(text: &str) -> Timestamp {
    text.parse().expect("valid instant")
}

async fn only_download(test: &TestStore, torrent_id: Uuid) -> Download {
    let mut downloads = test
        .store
        .downloads()
        .for_torrent(torrent_id)
        .await
        .expect("downloads");
    assert_eq!(downloads.len(), 1, "exactly one download");
    downloads.remove(0)
}

#[tokio::test]
async fn adding_after_the_torrent_was_deleted_reports_it_missing() {
    let test = TestStore::new().await;
    let torrent = test.add_torrent("hash").await;
    let loaded = test
        .store
        .torrents()
        .get(torrent.torrent_id)
        .await
        .expect("get");
    assert!(loaded.is_some());
    test.store
        .torrents()
        .delete(torrent.torrent_id)
        .await
        .expect("delete");

    let result = test
        .store
        .downloads()
        .try_add(torrent.torrent_id, &info("https://host/a"))
        .await;
    assert_eq!(result.expect("try add"), AddResult::TorrentMissing);
    let downloads = test
        .store
        .downloads()
        .for_torrent(torrent.torrent_id)
        .await
        .expect("list");
    assert_eq!(downloads, []);
}

#[tokio::test]
async fn deleting_survives_a_download_added_after_the_children_were_removed() {
    let test = TestStore::new().await;
    let torrent = test.add_torrent("hash").await;
    let downloads = test.store.downloads();
    downloads
        .try_add(torrent.torrent_id, &info("https://host/a"))
        .await
        .expect("first add");

    downloads
        .delete_for_torrent(torrent.torrent_id)
        .await
        .expect("delete children");
    let result = downloads
        .try_add(torrent.torrent_id, &info("https://host/b"))
        .await;
    assert_eq!(result.expect("late add"), AddResult::Added);
    test.store
        .torrents()
        .delete(torrent.torrent_id)
        .await
        .expect("delete torrent");

    assert_eq!(
        test.store
            .torrents()
            .get(torrent.torrent_id)
            .await
            .expect("get"),
        None
    );
    let remaining = downloads
        .for_torrent(torrent.torrent_id)
        .await
        .expect("list");
    assert_eq!(remaining, []);
}

#[tokio::test]
async fn adding_the_same_link_twice_keeps_one_download() {
    let test = TestStore::new().await;
    let torrent = test.add_torrent("hash").await;
    let downloads = test.store.downloads();

    let first = downloads
        .try_add(torrent.torrent_id, &info("https://host/a"))
        .await;
    let second = downloads
        .try_add(torrent.torrent_id, &info("https://host/a"))
        .await;
    assert_eq!(first.expect("first"), AddResult::Added);
    assert_eq!(second.expect("second"), AddResult::AlreadyExists);
    assert_eq!(
        downloads
            .for_torrent(torrent.torrent_id)
            .await
            .expect("list")
            .len(),
        1
    );
}

#[tokio::test]
async fn deleting_an_already_deleted_torrent_is_fine() {
    let test = TestStore::new().await;
    let torrent = test.add_torrent("hash").await;
    test.store
        .torrents()
        .delete(torrent.torrent_id)
        .await
        .expect("first delete");
    test.store
        .torrents()
        .delete(torrent.torrent_id)
        .await
        .expect("second delete");
}

#[tokio::test]
async fn blank_links_are_rejected() {
    let test = TestStore::new().await;
    let torrent = test.add_torrent("hash").await;
    for link in ["", "  "] {
        let result = test
            .store
            .downloads()
            .try_add(torrent.torrent_id, &info(link))
            .await;
        assert_eq!(
            result.expect("try add"),
            AddResult::InvalidInput,
            "link {link:?}"
        );
    }
}

#[tokio::test]
async fn added_downloads_are_queued_now_and_found_with_their_torrent() {
    let test = TestStore::new().await;
    let torrent = test.add_torrent("hash").await;
    let downloads = test.store.downloads();
    let before = Timestamp::now();
    downloads
        .try_add(torrent.torrent_id, &info("https://host/a"))
        .await
        .expect("add");

    let download = only_download(&test, torrent.torrent_id).await;
    assert_eq!(download.path, "https://host/a");
    assert_eq!(download.file_name.as_deref(), Some("file.mkv"));
    assert_eq!(download.torrent_id, torrent.torrent_id);
    assert_eq!(download.download_queued, Some(download.added));
    assert!(
        download.added
            >= before
                .checked_sub(jiff::SignedDuration::from_micros(1))
                .expect("range")
    );
    assert_eq!((download.retry_count, download.link.as_deref()), (0, None));

    let by_id = downloads
        .get(download.download_id)
        .await
        .expect("get")
        .expect("found");
    assert_eq!(by_id.download, download);
    assert_eq!(by_id.torrent.torrent_id, torrent.torrent_id);
    assert!(by_id.torrent.downloads.is_empty() && by_id.torrent.payload.is_none());
    let by_path = downloads
        .find(torrent.torrent_id, "https://host/a")
        .await
        .expect("find")
        .expect("found");
    assert_eq!(by_path, by_id);
    assert_eq!(
        downloads
            .find(torrent.torrent_id, "https://HOST/a")
            .await
            .expect("find"),
        None
    );
    assert_eq!(downloads.get(Uuid::new_v4()).await.expect("get"), None);
}

#[tokio::test]
async fn field_updates_set_their_column() {
    let test = TestStore::new().await;
    let torrent = test.add_torrent("hash").await;
    let downloads = test.store.downloads();
    downloads
        .try_add(torrent.torrent_id, &info("https://host/a"))
        .await
        .expect("add");
    let id = only_download(&test, torrent.torrent_id).await.download_id;
    let at = |second: u8| instant(&format!("2026-05-28T16:30:{second:02}Z"));

    downloads
        .update_link(id, "https://cdn/a")
        .await
        .expect("link");
    downloads
        .update_file_name(id, "renamed.mkv")
        .await
        .expect("file name");
    downloads
        .update_path(id, "https://host/b")
        .await
        .expect("path");
    downloads
        .update_download_started(id, Some(at(1)))
        .await
        .expect("started");
    downloads
        .update_download_finished(id, Some(at(2)))
        .await
        .expect("finished");
    downloads
        .update_unpacking_queued(id, Some(at(3)))
        .await
        .expect("unpack queued");
    downloads
        .update_unpacking_started(id, Some(at(4)))
        .await
        .expect("unpack started");
    downloads
        .update_unpacking_finished(id, Some(at(5)))
        .await
        .expect("unpack finished");
    downloads
        .update_completed(id, Some(at(6)))
        .await
        .expect("completed");
    downloads
        .update_error(id, Some("boom"))
        .await
        .expect("error");
    downloads
        .update_retry_count(id, 4)
        .await
        .expect("retry count");
    downloads
        .update_remote_id(id, "gid-1")
        .await
        .expect("remote id");

    let download = only_download(&test, torrent.torrent_id).await;
    assert_eq!(download.link.as_deref(), Some("https://cdn/a"));
    assert_eq!(download.file_name.as_deref(), Some("renamed.mkv"));
    assert_eq!(download.path, "https://host/b");
    assert_eq!(download.download_started, Some(at(1)));
    assert_eq!(download.download_finished, Some(at(2)));
    assert_eq!(download.unpacking_queued, Some(at(3)));
    assert_eq!(download.unpacking_started, Some(at(4)));
    assert_eq!(download.unpacking_finished, Some(at(5)));
    assert_eq!(download.completed, Some(at(6)));
    assert_eq!(download.error.as_deref(), Some("boom"));
    assert_eq!(download.retry_count, 4);
    assert_eq!(download.remote_id.as_deref(), Some("gid-1"));

    downloads
        .update_completed(id, None)
        .await
        .expect("clear completed");
    downloads.update_error(id, None).await.expect("clear error");
    let download = only_download(&test, torrent.torrent_id).await;
    assert_eq!((download.completed, download.error), (None, None));
    downloads
        .update_link(Uuid::new_v4(), "x")
        .await
        .expect("missing downloads are ignored");
}

#[tokio::test]
async fn reset_queues_the_download_again_and_keeps_its_identity() {
    let test = TestStore::new().await;
    let torrent = test.add_torrent("hash").await;
    let downloads = test.store.downloads();
    downloads
        .try_add(torrent.torrent_id, &info("https://host/a"))
        .await
        .expect("add");
    let id = only_download(&test, torrent.torrent_id).await.download_id;
    let at = instant("2026-05-28T16:30:00Z");
    downloads
        .update_link(id, "https://cdn/a")
        .await
        .expect("link");
    downloads
        .update_remote_id(id, "gid-1")
        .await
        .expect("remote id");
    downloads
        .update_download_started(id, Some(at))
        .await
        .expect("started");
    downloads
        .update_unpacking_finished(id, Some(at))
        .await
        .expect("unpacked");
    downloads
        .update_completed(id, Some(at))
        .await
        .expect("completed");
    downloads
        .update_error(id, Some("boom"))
        .await
        .expect("error");
    downloads
        .update_retry_count(id, 2)
        .await
        .expect("retry count");

    let queued = instant("2030-01-01T00:00:00Z");
    downloads.reset(id, Some(queued)).await.expect("reset");
    let download = only_download(&test, torrent.torrent_id).await;
    let expected = Download {
        download_id: id,
        torrent_id: torrent.torrent_id,
        path: "https://host/a".to_owned(),
        file_name: Some("file.mkv".to_owned()),
        remote_id: Some("gid-1".to_owned()),
        added: download.added,
        download_queued: Some(queued),
        ..Download::default()
    };
    assert_eq!(download, expected);

    downloads
        .reset(id, None)
        .await
        .expect("reset without a queue time");
    let download = only_download(&test, torrent.torrent_id).await;
    assert_eq!(download.download_queued, Some(download.added));

    let missing = Uuid::new_v4();
    let error = downloads
        .reset(missing, None)
        .await
        .expect_err("missing download");
    assert!(
        matches!(error, StoreError::DownloadNotFound(id) if id == missing),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        format!("Cannot find download with ID {missing}")
    );
}

#[tokio::test]
async fn deleting_the_downloads_of_a_torrent_leaves_other_torrents_alone() {
    let test = TestStore::new().await;
    let first = test.add_torrent("first").await;
    let second = test.add_torrent("second").await;
    let downloads = test.store.downloads();
    downloads
        .try_add(first.torrent_id, &info("https://host/a"))
        .await
        .expect("add first");
    downloads
        .try_add(second.torrent_id, &info("https://host/a"))
        .await
        .expect("add second");

    downloads
        .delete_for_torrent(first.torrent_id)
        .await
        .expect("delete");
    let remaining = downloads
        .for_torrent(first.torrent_id)
        .await
        .expect("first");
    assert_eq!(remaining, []);
    assert_eq!(
        downloads
            .for_torrent(second.torrent_id)
            .await
            .expect("second")
            .len(),
        1
    );
    let listed = test.store.torrents().list().await.expect("list");
    let counts: Vec<(String, usize)> = listed
        .into_iter()
        .map(|torrent| (torrent.hash, torrent.downloads.len()))
        .collect();
    assert_eq!(counts, [("first".to_owned(), 0), ("second".to_owned(), 1)]);
}

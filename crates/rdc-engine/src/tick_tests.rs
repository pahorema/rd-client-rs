use super::*;
use crate::{
    TorrentTemplate,
    testing::{
        FakeDebridProvider, ProviderCall, ProviderOperation, TestEngine,
        downloads::{FakeDownloadEngine, FakeDownloadFactory},
    },
};
use rdc_debrid::{DebridError, RemoteTorrent};
use rdc_model::{DownloadInfo, RemoteFile, TorrentFinishedAction, settings::Settings};
use tokio::fs;

pub(super) const HASH: &str = "0123456789012345678901234567890123456789";
const HASH_TWO: &str = "1123456789012345678901234567890123456789";

pub(super) async fn fixture() -> (TestEngine, FakeDownloadFactory) {
    let fake = FakeDebridProvider::default();
    let factory = FakeDownloadFactory::new();
    let mut options = fake.options();
    options.download_factory = Some(Arc::new(factory.clone()));
    let mut settings = Settings::default();
    settings.provider.api_key = "fake-key".to_owned();
    settings.general.download_limit = 1;
    (
        TestEngine::with_options(settings, fake, options)
            .await
            .expect("engine"),
        factory,
    )
}

pub(super) async fn add(
    test: &TestEngine,
    hash: &str,
    id: &str,
    names: &[&str],
    template: TorrentTemplate,
) -> Torrent {
    test.fake.queue_add(RemoteTorrent {
        id: id.to_owned(),
        hash: hash.to_owned(),
        filename: "Example".to_owned(),
        status: Some("finished".to_owned()),
        files: Some(
            names
                .iter()
                .enumerate()
                .map(|(index, name)| RemoteFile {
                    id: i64::try_from(index).expect("index"),
                    path: (*name).to_owned(),
                    bytes: 8,
                    selected: true,
                    download_link: Some(format!("https://example.invalid/{name}")),
                })
                .collect(),
        ),
        ..RemoteTorrent::default()
    });
    test.fake
        .set_selection(id.to_owned(), Some(names.len().max(1)));
    test.fake.set_download_infos(
        id.to_owned(),
        Some(
            names
                .iter()
                .map(|name| DownloadInfo {
                    file_name: Some((*name).to_owned()),
                    restricted_link: format!("https://example.invalid/{name}"),
                })
                .collect(),
        ),
    );
    test.engine
        .add_magnet(&format!("magnet:?xt=urn:btih:{hash}&dn=Example"), template)
        .await
        .expect("add")
}
pub(super) fn template() -> TorrentTemplate {
    TorrentTemplate {
        finished_action: TorrentFinishedAction::None,
        ..TorrentTemplate::default()
    }
}
pub(super) async fn current(test: &TestEngine, id: Uuid) -> Torrent {
    test.store
        .torrents()
        .get(id)
        .await
        .expect("read")
        .expect("torrent")
}
pub(super) async fn finish(
    test: &TestEngine,
    factory: &FakeDownloadFactory,
    index: usize,
    content: &[u8],
) {
    let request = factory.requests()[index].clone();
    fs::write(&request.local_path, content)
        .await
        .expect("download file");
    let backend = factory.engines()[index].clone();
    backend.set_progress(rdc_download::Progress {
        bytes_done: u64::try_from(content.len()).expect("length"),
        bytes_total: 0,
        speed: 5,
    });
    backend.complete(Ok(()));
    let active = test
        .engine
        .inner
        .registry
        .downloads
        .lock()
        .expect("registry")
        .get(&request.download.download_id)
        .expect("active")
        .clone();
    tokio::time::timeout(Duration::from_secs(2), active.engine.completion().wait())
        .await
        .expect("completion timeout")
        .expect("completion");
}

#[tokio::test]
async fn selection_creation_transfer_and_completion_advance_without_extra_selection_ticks() {
    let (test, factory) = fixture().await;
    let torrent = add(&test, HASH, "one", &["file.bin"], template()).await;
    test.engine.tick_once().await.expect("selection tick");
    let selected = current(&test, torrent.torrent_id).await;
    assert!(selected.files_selected.is_some());
    assert_eq!(selected.downloads.len(), 1);
    assert!(selected.downloads[0].download_started.is_none());
    test.engine.tick_once().await.expect("start tick");
    assert_eq!(factory.engines().len(), 1);
    finish(&test, &factory, 0, b"complete").await;
    test.engine.tick_once().await.expect("completion tick");
    let complete = current(&test, torrent.torrent_id).await;
    assert!(complete.completed.is_some());
    assert!(complete.error.is_none());
    assert!(complete.downloads[0].download_finished.is_some());
    assert!(complete.downloads[0].unpacking_finished.is_some());
    assert_eq!(test.engine.session_downloaded_bytes(), 8);
    assert_eq!(test.engine.active_download_speed_sum(), 0);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn a_full_download_slot_does_not_starve_selection_or_completion_of_other_torrents() {
    let (test, factory) = fixture().await;
    let first = add(&test, HASH, "one", &["a.bin", "b.bin"], template()).await;
    test.engine.tick_once().await.expect("create");
    let mut second_template = template();
    second_template.host_download_action = TorrentHostDownloadAction::DownloadNone;
    let second = add(&test, HASH_TWO, "two", &["other.bin"], second_template).await;
    test.engine.tick_once().await.expect("limited tick");
    assert_eq!(factory.engines().len(), 1);
    assert!(current(&test, second.torrent_id).await.completed.is_some());
    assert_eq!(
        current(&test, first.torrent_id)
            .await
            .downloads
            .iter()
            .filter(|download| download.download_started.is_some())
            .count(),
        1
    );
    test.engine.shutdown().await;
}

#[tokio::test]
async fn low_disk_pause_blocks_only_new_internal_starts_and_not_other_torrent_phases() {
    let (test, factory) = fixture().await;
    let first = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.engine.tick_once().await.expect("create");
    test.engine.inner.low_disk.store(true, Ordering::Relaxed);
    let mut second_template = template();
    second_template.host_download_action = TorrentHostDownloadAction::DownloadNone;
    let second = add(&test, HASH_TWO, "two", &["other.bin"], second_template).await;
    test.engine.tick_once().await.expect("paused");
    assert_eq!(factory.engines().len(), 0);
    assert!(
        current(&test, first.torrent_id).await.downloads[0]
            .download_started
            .is_none()
    );
    assert!(current(&test, second.torrent_id).await.completed.is_some());
    test.engine.inner.low_disk.store(false, Ordering::Relaxed);
    test.engine.tick_once().await.expect("resume");
    assert_eq!(factory.engines().len(), 1);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn rate_limited_unrestrict_requeues_at_cooldown_without_consuming_a_retry() {
    let (test, factory) = fixture().await;
    let torrent = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.engine.tick_once().await.expect("create");
    test.fake
        .queue_rate_limit(ProviderOperation::Unrestrict, Duration::from_secs(60));
    test.engine.tick_once().await.expect("cooldown");
    let torrent = current(&test, torrent.torrent_id).await;
    let download = &torrent.downloads[0];
    assert_eq!(download.retry_count, 0);
    assert!(download.error.is_none());
    assert!(download.completed.is_none());
    assert!(download.download_started.is_none());
    assert_eq!(
        download.download_queued,
        test.engine.rate_limit_status().next_dequeue_time
    );
    assert_eq!(factory.requests().len(), 0);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn ordinary_unrestrict_failure_uses_backoff_without_aborting_other_phases() {
    let (test, factory) = fixture().await;
    let first = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.engine.tick_once().await.expect("create");
    let mut other_template = template();
    other_template.host_download_action = TorrentHostDownloadAction::DownloadNone;
    let other = add(&test, HASH_TWO, "two", &["other.bin"], other_template).await;
    test.fake.queue_failure(
        ProviderOperation::Unrestrict,
        DebridError::Provider("failed link".to_owned()),
    );
    test.engine.tick_once().await.expect("failure tick");
    let first = current(&test, first.torrent_id).await;
    assert_eq!(first.downloads[0].retry_count, 1);
    assert!(
        first.downloads[0].download_queued.expect("queued")
            > add_duration(Timestamp::now(), Duration::from_secs(10))
    );
    assert!(current(&test, other.torrent_id).await.completed.is_some());
    assert_eq!(factory.requests().len(), 0);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn failed_and_empty_id_starts_release_the_slot_for_the_next_download() {
    for start in [Err("start failed".to_owned()), Ok(String::new())] {
        let (test, factory) = fixture().await;
        let bad = FakeDownloadEngine::new();
        bad.set_start_result(start);
        factory.push(bad);
        let mut options = template();
        options.download_retry_attempts = 0;
        options.torrent_retry_attempts = 0;
        let torrent = add(&test, HASH, "one", &["a.bin", "b.bin"], options).await;
        test.engine.tick_once().await.expect("create");
        test.engine.tick_once().await.expect("starts");
        let torrent = current(&test, torrent.torrent_id).await;
        assert_eq!(factory.engines().len(), 2);
        assert!(torrent.downloads[0].completed.is_some());
        assert!(torrent.downloads[0].error.is_some());
        assert!(torrent.downloads[1].download_started.is_some());
        assert_eq!(
            test.engine
                .inner
                .registry
                .downloads
                .lock()
                .expect("registry")
                .len(),
            1
        );
        test.engine.shutdown().await;
    }
}

#[tokio::test]
async fn transfer_failure_retries_with_a_refreshed_restricted_link_and_preserved_count() {
    let (test, factory) = fixture().await;
    let torrent = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.engine.tick_once().await.expect("create");
    test.engine.tick_once().await.expect("start");
    let backend = factory.engines()[0].clone();
    backend.complete(Err("connection lost".to_owned()));
    let active = test
        .engine
        .inner
        .registry
        .downloads
        .lock()
        .expect("registry")
        .values()
        .next()
        .expect("active")
        .clone();
    assert!(active.engine.completion().wait().await.is_err());
    test.fake.set_download_infos(
        "one".to_owned(),
        Some(vec![DownloadInfo {
            file_name: Some("a.bin".to_owned()),
            restricted_link: "https://refreshed.invalid/a.bin".to_owned(),
        }]),
    );
    test.engine.tick_once().await.expect("retry tick");
    let torrent = current(&test, torrent.torrent_id).await;
    assert_eq!(torrent.downloads[0].retry_count, 1);
    assert_eq!(torrent.downloads[0].path, "https://refreshed.invalid/a.bin");
    assert_eq!(factory.engines().len(), 2);
    assert!(test.fake.calls().iter().any(|call| matches!(call, ProviderCall::Unrestrict { link, .. } if link == "https://refreshed.invalid/a.bin")));
    test.engine.shutdown().await;
}

#[tokio::test]
async fn completed_error_torrents_are_neither_requeued_nor_deleted_before_retention_expires() {
    let (test, _) = fixture().await;
    test.engine
        .update_setting("Provider:ApiKey", Some(""))
        .await
        .expect("disable dispatch");
    let torrent = test
        .engine
        .add_magnet(
            &format!("magnet:?xt=urn:btih:{HASH}&dn=Example"),
            TorrentTemplate {
                delete_on_error: 10,
                ..template()
            },
        )
        .await
        .expect("queued");
    let completed = Timestamp::now()
        .checked_sub(SignedDuration::from_secs(5 * 60))
        .expect("time");
    test.store
        .torrents()
        .update_complete(
            torrent.torrent_id,
            Some("Could not add to provider: rejected"),
            Some(completed),
            RetryOnError::Skip,
        )
        .await
        .expect("completed error");
    test.engine
        .update_setting("Provider:ApiKey", Some("fake-key"))
        .await
        .expect("enable");
    test.engine.tick_once().await.expect("tick");
    let torrent = current(&test, torrent.torrent_id).await;
    assert_eq!(torrent.completed, Some(completed));
    assert!(torrent.rd_id.is_none());
    assert_eq!(test.fake.calls(), Vec::new());
    test.engine.shutdown().await;
}

#[tokio::test]
async fn all_files_excluded_is_final_and_does_not_schedule_a_torrent_retry() {
    let (test, _) = fixture().await;
    let torrent = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.fake.set_selection("one".to_owned(), Some(0));
    test.engine.tick_once().await.expect("selection");
    let torrent = current(&test, torrent.torrent_id).await;
    assert_eq!(torrent.error.as_deref(), Some("All files excluded"));
    assert!(torrent.completed.is_some());
    assert!(torrent.retry.is_none());
    assert_eq!(torrent.retry_count, torrent.torrent_retry_attempts);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn finished_actions_remove_the_provider_and_local_rows_but_keep_local_files() {
    let (test, factory) = fixture().await;
    let torrent = add(
        &test,
        HASH,
        "one",
        &["file.bin"],
        TorrentTemplate {
            finished_action: TorrentFinishedAction::RemoveAllTorrents,
            ..template()
        },
    )
    .await;
    test.engine.tick_once().await.expect("create");
    test.engine.tick_once().await.expect("start");
    finish(&test, &factory, 0, b"complete").await;
    let output = factory.requests()[0].local_path.clone();
    test.engine.tick_once().await.expect("complete");
    assert!(current(&test, torrent.torrent_id).await.completed.is_some());
    test.engine.tick_once().await.expect("finished action");
    assert!(
        test.store
            .torrents()
            .get(torrent.torrent_id)
            .await
            .expect("read")
            .is_none()
    );
    assert!(output.exists());
    assert!(
        test.fake
            .calls()
            .iter()
            .any(|call| matches!(call, ProviderCall::Delete { .. }))
    );
    test.engine.shutdown().await;
}

#[tokio::test]
async fn concurrent_ticks_start_a_download_only_once_and_shutdown_cancels_pending_starts() {
    let (test, factory) = fixture().await;
    add(&test, HASH, "one", &["file.bin"], template()).await;
    test.engine.tick_once().await.expect("create");
    let (first, second) = tokio::join!(test.engine.tick_once(), test.engine.tick_once());
    first.expect("first");
    second.expect("second");
    assert_eq!(factory.engines().len(), 1);
    assert_eq!(factory.engines()[0].started(), 1);
    test.engine.shutdown().await;
    assert_eq!(factory.engines()[0].cancelled(), 1);
    test.engine.tick_once().await.expect("shutdown tick");
    assert_eq!(factory.engines().len(), 1);
}

#[tokio::test]
async fn zip_torrent_advances_through_extraction_completion_and_finished_action() {
    use std::io::{Cursor, Write};
    let (test, factory) = fixture().await;
    let torrent = add(
        &test,
        HASH,
        "one",
        &["archive.zip"],
        TorrentTemplate {
            finished_action: TorrentFinishedAction::RemoveAllTorrents,
            ..template()
        },
    )
    .await;
    test.engine.tick_once().await.expect("selection");
    test.engine.tick_once().await.expect("start");
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("content.txt", zip::write::SimpleFileOptions::default())
        .expect("entry");
    zip.write_all(b"expanded").expect("content");
    let bytes = zip.finish().expect("zip").into_inner();
    finish(&test, &factory, 0, &bytes).await;
    test.engine.tick_once().await.expect("start extraction");
    assert!(current(&test, torrent.torrent_id).await.completed.is_none());
    let active = test
        .engine
        .inner
        .registry
        .unpacks
        .lock()
        .expect("registry")
        .values()
        .next()
        .expect("unpack")
        .clone();
    active.completion.clone().wait().await.expect("extraction");
    test.engine.tick_once().await.expect("complete");
    assert!(current(&test, torrent.torrent_id).await.completed.is_some());
    let expanded = factory.requests()[0]
        .local_path
        .parent()
        .expect("directory")
        .join("content.txt");
    assert_eq!(fs::read(&expanded).await.expect("expanded"), b"expanded");
    test.engine.tick_once().await.expect("finished action");
    assert!(
        test.store
            .torrents()
            .get(torrent.torrent_id)
            .await
            .expect("read")
            .is_none()
    );
    assert!(expanded.exists());
    test.engine.shutdown().await;
}

#[tokio::test]
async fn tick_holds_multi_volume_extraction_while_a_sibling_transfer_is_active() {
    let (test, factory) = fixture().await;
    let torrent = add(
        &test,
        HASH,
        "one",
        &["archive.part1.rar", "archive.part2.rar"],
        TorrentTemplate {
            torrent_retry_attempts: 0,
            ..template()
        },
    )
    .await;
    test.engine.tick_once().await.expect("create");
    test.engine.tick_once().await.expect("first transfer");
    finish(&test, &factory, 0, b"bad rar1").await;
    test.engine.tick_once().await.expect("second transfer");
    assert_eq!(factory.engines().len(), 2);
    assert_eq!(
        test.engine
            .inner
            .registry
            .unpacks
            .lock()
            .expect("registry")
            .len(),
        0
    );
    assert!(
        current(&test, torrent.torrent_id).await.downloads[0]
            .unpacking_started
            .is_none()
    );
    finish(&test, &factory, 1, b"bad rar2").await;
    test.engine.tick_once().await.expect("start set extraction");
    let active = {
        let entries = test.engine.inner.registry.unpacks.lock().expect("registry");
        assert_eq!(entries.len(), 2);
        entries.values().next().expect("unpack").clone()
    };
    assert!(active.completion.clone().wait().await.is_err());
    test.engine
        .tick_once()
        .await
        .expect("complete failed extraction");
    let torrent = current(&test, torrent.torrent_id).await;
    assert!(torrent.completed.is_some());
    assert_eq!(
        torrent.error.as_deref(),
        Some("2/2 downloads failed with errors")
    );
    test.engine.shutdown().await;
}

#[tokio::test]
async fn automatic_torrent_retry_sets_the_consumed_count_before_an_immediate_add_failure() {
    let (test, _) = fixture().await;
    let torrent = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.fake.set_status("one".to_owned(), TorrentStatus::Error);
    test.engine
        .torrent(torrent.torrent_id)
        .await
        .expect("refresh");
    test.engine.tick_once().await.expect("first error");
    let failed = current(&test, torrent.torrent_id).await;
    assert_eq!(failed.retry_count, 1);
    assert!(failed.retry.is_some());
    test.fake.queue_failure(
        ProviderOperation::AddMagnet,
        DebridError::Provider("provider rejected retry".to_owned()),
    );
    test.engine.tick_once().await.expect("retry");
    let torrents = test.engine.torrents().await.expect("list");
    assert_eq!(torrents.len(), 1);
    assert_ne!(torrents[0].torrent_id, torrent.torrent_id);
    assert_eq!(torrents[0].retry_count, 1);
    assert!(torrents[0].retry.is_none());
    assert!(torrents[0].completed.is_some());
    assert!(
        torrents[0]
            .error
            .as_deref()
            .expect("error")
            .contains("provider rejected retry")
    );
    test.engine.tick_once().await.expect("no further retry");
    assert_eq!(
        test.engine.torrents().await.expect("list")[0].torrent_id,
        torrents[0].torrent_id
    );
    test.engine.shutdown().await;
}

#[tokio::test]
async fn exhausted_download_retries_complete_the_torrent_with_an_aggregate_error() {
    let (test, factory) = fixture().await;
    let torrent = add(
        &test,
        HASH,
        "one",
        &["a.bin"],
        TorrentTemplate {
            download_retry_attempts: 0,
            torrent_retry_attempts: 0,
            ..template()
        },
    )
    .await;
    test.engine.tick_once().await.expect("create");
    test.engine.tick_once().await.expect("start");
    factory.engines()[0].complete(Err("transfer failed".to_owned()));
    let active = test
        .engine
        .inner
        .registry
        .downloads
        .lock()
        .expect("registry")
        .values()
        .next()
        .expect("transfer")
        .clone();
    assert!(active.engine.completion().wait().await.is_err());
    test.engine.tick_once().await.expect("failure completion");
    let torrent = current(&test, torrent.torrent_id).await;
    assert_eq!(
        torrent.error.as_deref(),
        Some("1/1 downloads failed with errors")
    );
    assert!(torrent.completed.is_some());
    assert!(torrent.retry.is_none());
    test.engine.shutdown().await;
}

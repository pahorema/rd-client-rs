use super::*;
use crate::testing::{ProviderCall, ProviderOperation, TestEngine};
use rdc_debrid::{DebridError, RemoteTorrent};
use rdc_model::{
    DownloadClient, DownloadInfo, RemoteFile, TorrentHostDownloadAction, settings::Settings,
};

const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Movie";

fn settings() -> Settings {
    let mut settings = Settings::default();
    settings.provider.api_key = "test-key".into();
    settings
}

fn remote(id: &str) -> RemoteTorrent {
    RemoteTorrent {
        id: id.into(),
        filename: "Movie".into(),
        hash: HASH.into(),
        status: Some("downloading".into()),
        ..RemoteTorrent::default()
    }
}

#[tokio::test]
async fn successful_add_dispatches_and_refreshes_immediately_without_duplicate_mutation() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    test.fake.queue_add(remote("first"));
    let added = test
        .engine
        .add_magnet(
            MAGNET,
            TorrentTemplate {
                category: Some("movies".into()),
                priority: Some(2),
                ..TorrentTemplate::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(added.rd_id.as_deref(), Some("first"));
    assert_eq!(added.rd_name.as_deref(), Some("Movie"));
    assert!(
        test.fake
            .calls()
            .iter()
            .any(|call| matches!(call, ProviderCall::Refresh { .. }))
    );
    let before = test
        .store
        .torrents()
        .get(added.torrent_id)
        .await
        .unwrap()
        .unwrap();
    let duplicate = test
        .engine
        .add_magnet(
            MAGNET,
            TorrentTemplate {
                category: Some("ignored".into()),
                ..TorrentTemplate::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(duplicate.torrent_id, added.torrent_id);
    assert_eq!(
        test.store
            .torrents()
            .get(added.torrent_id)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        test.fake
            .calls()
            .iter()
            .filter(|call| matches!(call, ProviderCall::AddMagnet(_)))
            .count(),
        1
    );
    let listed = test.engine.torrents().await.unwrap();
    assert!(listed[0].payload.is_none());
    assert_eq!(
        test.engine
            .torrent_by_hash(&HASH.to_uppercase())
            .await
            .unwrap()
            .unwrap()
            .torrent_id,
        added.torrent_id
    );
    assert!(
        test.engine
            .torrent(added.torrent_id)
            .await
            .unwrap()
            .unwrap()
            .payload
            .is_some()
    );
}

#[tokio::test]
async fn concurrent_additions_share_duplicate_check_and_dispatch() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    let (first, second) = tokio::join!(
        test.engine.add_magnet(MAGNET, TorrentTemplate::default()),
        test.engine.add_magnet(MAGNET, TorrentTemplate::default())
    );
    assert_eq!(first.unwrap().torrent_id, second.unwrap().torrent_id);
    assert_eq!(test.engine.torrents().await.unwrap().len(), 1);
    assert_eq!(
        test.fake
            .calls()
            .iter()
            .filter(|call| matches!(call, ProviderCall::AddMagnet(_)))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_add_whose_caller_goes_away_still_stores_the_provider_id() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    test.fake.queue_add(remote("held"));
    test.fake.hold_adds();
    let engine = test.engine.clone();
    let caller =
        tokio::spawn(async move { engine.add_magnet(MAGNET, TorrentTemplate::default()).await });
    let adding = |calls: &[ProviderCall]| {
        calls
            .iter()
            .filter(|call| matches!(call, ProviderCall::AddMagnet(_)))
            .count()
    };
    time::timeout(Duration::from_secs(5), async {
        while adding(&test.fake.calls()) == 0 {
            time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    test.fake.release_adds();
    let torrents = time::timeout(Duration::from_secs(5), async {
        loop {
            let torrents = test.engine.torrents().await.unwrap();
            if torrents[0].rd_id.is_some() {
                break torrents;
            }
            time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(torrents.len(), 1);
    assert_eq!(torrents[0].rd_id.as_deref(), Some("held"));
    assert_eq!(adding(&test.fake.calls()), 1);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn provider_limit_counts_sent_rows_even_before_status_changes() {
    let mut settings = settings();
    settings.provider.max_parallel_downloads = 1;
    let test = TestEngine::with_settings(settings).await.unwrap();
    test.engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    let second = test
        .engine
        .add_magnet(
            "magnet:?xt=urn:btih:1123456789abcdef0123456789abcdef01234567",
            TorrentTemplate::default(),
        )
        .await
        .unwrap();
    assert!(second.rd_id.is_none());
    assert_eq!(second.rd_status, Some(TorrentStatus::Queued));
    test.engine.dispatch_queued().await.unwrap();
    assert_eq!(
        test.fake
            .calls()
            .iter()
            .filter(|call| matches!(call, ProviderCall::AddMagnet(_)))
            .count(),
        1
    );
}

#[tokio::test]
async fn cooldown_preserves_queue_and_retry_allowance() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    test.fake.queue_failure(
        ProviderOperation::AddMagnet,
        DebridError::RateLimited {
            message: "slow down".into(),
            retry_after: Duration::from_secs(60),
        },
    );
    let queued = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    assert!(queued.rd_id.is_none());
    assert!(queued.completed.is_none());
    assert!(queued.retry.is_none());
    assert_eq!(queued.retry_count, 0);
    test.engine.dispatch_queued().await.unwrap();
    assert_eq!(
        test.fake
            .calls()
            .iter()
            .filter(|call| matches!(call, ProviderCall::AddMagnet(_)))
            .count(),
        1
    );
}

#[tokio::test]
async fn invalid_and_banned_input_never_reaches_provider() {
    let mut settings = settings();
    settings.general.banned_trackers = Some(" private.example ,,".into());
    let test = TestEngine::with_settings(settings).await.unwrap();
    assert!(
        test.engine
            .add_magnet("magnet:?dn=missing", TorrentTemplate::default())
            .await
            .is_err()
    );
    assert!(
        test.engine
            .add_torrent_file(b"invalid", TorrentTemplate::default())
            .await
            .is_err()
    );
    assert!(
        test.engine
            .add_magnet(
                &format!("{MAGNET}&tr=https%3A%2F%2FPRIVATE.example%2Fannounce"),
                TorrentTemplate::default()
            )
            .await
            .is_err()
    );
    assert_eq!(test.engine.torrents().await.unwrap(), Vec::new());
    assert!(!test.fake.calls().iter().any(|call| matches!(
        call,
        ProviderCall::AddMagnet(_) | ProviderCall::AddTorrentFile(_)
    )));
}

#[tokio::test]
async fn enrichment_fetch_failure_still_adds_and_copies_the_payload() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let mut settings = settings();
    settings.general.tracker_enrichment_list = Some(server.uri());
    settings.general.copy_added_torrents = Some(directory.path().to_string_lossy().into_owned());
    let test = TestEngine::with_settings(settings).await.unwrap();
    let added = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    assert_eq!(added.payload.as_deref(), Some(MAGNET));
    assert_eq!(
        fs::read_to_string(directory.path().join("Movie.magnet"))
            .await
            .unwrap(),
        MAGNET
    );
}

#[tokio::test]
async fn enrichment_cache_is_shared_and_encoded_names_remain_intact() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("udp://tracker.example/announce\n"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut settings = settings();
    settings.general.tracker_enrichment_list = Some(server.uri());
    let test = TestEngine::with_settings(settings).await.unwrap();
    let magnet = format!("{MAGNET}%20%26%20More");
    let added = test
        .engine
        .add_magnet(&magnet, TorrentTemplate::default())
        .await
        .unwrap();
    let parsed = MagnetLink::parse(added.payload.as_deref().unwrap()).unwrap();
    assert_eq!(parsed.display_name.as_deref(), Some("Movie & More"));
    assert_eq!(parsed.trackers, ["udp://tracker.example/announce"]);
    test.engine
        .add_magnet(
            "magnet:?xt=urn:btih:1123456789abcdef0123456789abcdef01234567",
            TorrentTemplate::default(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn torrent_metadata_add_keeps_the_submitted_info_hash() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    let bytes =
        b"d4:infod6:lengthi1e4:name5:Movie12:piece lengthi1e6:pieces20:abcdefghijklmnopqrstee";
    let parsed = TorrentFile::parse(bytes).unwrap();
    let added = test
        .engine
        .add_torrent_file(bytes, TorrentTemplate::default())
        .await
        .unwrap();
    assert_eq!(added.hash, parsed.info_hash_hex());
    assert!(added.is_file);
    assert_eq!(
        BASE64
            .decode(added.payload.as_deref().unwrap().as_bytes())
            .unwrap(),
        bytes
    );
    assert!(
        test.fake
            .calls()
            .iter()
            .any(|call| matches!(call, ProviderCall::AddTorrentFile(payload) if payload == bytes))
    );
}

#[tokio::test]
async fn retry_readds_stored_payload_and_preserves_configuration_and_count() {
    let mut settings = settings();
    settings.provider.api_key.clear();
    let test = TestEngine::with_settings(settings).await.unwrap();
    let added = test
        .engine
        .add_magnet(
            MAGNET,
            TorrentTemplate {
                category: Some("movies".into()),
                priority: Some(4),
                download_client: DownloadClient::Symlink,
                ..TorrentTemplate::default()
            },
        )
        .await
        .unwrap();
    test.store
        .torrents()
        .update_retry(added.torrent_id, Some(Timestamp::now()), 3)
        .await
        .unwrap();
    test.engine
        .retry_torrent(added.torrent_id, 3)
        .await
        .unwrap();
    assert!(
        test.store
            .torrents()
            .get(added.torrent_id)
            .await
            .unwrap()
            .is_none()
    );
    let replacement = test.engine.torrent_by_hash(HASH).await.unwrap().unwrap();
    assert_ne!(replacement.torrent_id, added.torrent_id);
    assert_eq!(replacement.retry_count, 3);
    assert!(replacement.retry.is_none());
    assert_eq!(replacement.category, added.category);
    assert_eq!(replacement.priority, added.priority);
    assert_eq!(replacement.download_client, added.download_client);
    test.engine.retry(replacement.torrent_id).await.unwrap();
    assert_eq!(
        test.engine
            .torrent_by_hash(HASH)
            .await
            .unwrap()
            .unwrap()
            .retry_count,
        0
    );
}

#[tokio::test]
async fn retry_download_removes_file_resets_progress_and_reopens_parent() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = settings();
    settings.provider.api_key.clear();
    settings.download_client.download_path = directory.path().to_string_lossy().into_owned();
    let test = TestEngine::with_settings(settings).await.unwrap();
    let added = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    test.store
        .downloads()
        .try_add(
            added.torrent_id,
            &DownloadInfo {
                file_name: Some("file.mkv".into()),
                restricted_link: "restricted".into(),
            },
        )
        .await
        .unwrap();
    let download = test
        .store
        .downloads()
        .for_torrent(added.torrent_id)
        .await
        .unwrap()
        .remove(0);
    let id = download.download_id;
    test.store
        .downloads()
        .update_link(id, "https://download.example/file.mkv")
        .await
        .unwrap();
    test.store
        .downloads()
        .update_remote_id(id, "remote-transfer")
        .await
        .unwrap();
    test.store
        .downloads()
        .update_retry_count(id, 2)
        .await
        .unwrap();
    test.store
        .downloads()
        .update_error(id, Some("failed"))
        .await
        .unwrap();
    test.store
        .downloads()
        .update_completed(id, Some(Timestamp::now()))
        .await
        .unwrap();
    test.store
        .torrents()
        .update_complete(
            added.torrent_id,
            Some("failed"),
            Some(Timestamp::now()),
            RetryOnError::Skip,
        )
        .await
        .unwrap();
    fs::create_dir_all(directory.path().join("Movie"))
        .await
        .unwrap();
    fs::write(directory.path().join("Movie/file.mkv"), "partial")
        .await
        .unwrap();
    test.engine.retry_download(id).await.unwrap();
    assert!(!directory.path().join("Movie/file.mkv").exists());
    let reset = test.store.downloads().get(id).await.unwrap().unwrap();
    assert_eq!(reset.download.retry_count, 0);
    assert!(reset.download.link.is_none());
    assert!(reset.download.error.is_none());
    assert!(reset.download.completed.is_none());
    assert!(reset.download.download_queued.is_some());
    assert_eq!(reset.download.remote_id.as_deref(), Some("remote-transfer"));
    assert!(reset.torrent.completed.is_none());
    assert!(reset.torrent.error.is_none());
}

#[tokio::test]
async fn update_writes_only_editable_fields_and_selection_uses_indices() {
    let mut settings = settings();
    settings.provider.api_key.clear();
    let test = TestEngine::with_settings(settings).await.unwrap();
    let mut added = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    added.set_files(&[RemoteFile {
        id: 42,
        path: "file.mkv".into(),
        bytes: 1,
        selected: false,
        download_link: None,
    }]);
    test.store.torrents().update_rd_data(&added).await.unwrap();
    let update = TorrentUpdate {
        download_client: DownloadClient::Symlink,
        host_download_action: TorrentHostDownloadAction::DownloadNone,
        category: Some("updated".into()),
        priority: Some(1),
        download_retry_attempts: 8,
        torrent_retry_attempts: 9,
        delete_on_error: 10,
        lifetime: 11,
    };
    test.engine
        .update(added.torrent_id, update.clone())
        .await
        .unwrap();
    let changed = test
        .store
        .torrents()
        .get(added.torrent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.download_client, update.download_client);
    assert_eq!(changed.host_download_action, update.host_download_action);
    assert_eq!(changed.category, update.category);
    assert_eq!(changed.priority, update.priority);
    assert_eq!(changed.download_retry_attempts, 8);
    assert_eq!(changed.torrent_retry_attempts, 9);
    assert_eq!(changed.delete_on_error, 10);
    assert_eq!(changed.lifetime, 11);
    assert_eq!(changed.added, added.added);
    assert_eq!(changed.hash, added.hash);
    assert_eq!(changed.rd_files, added.rd_files);
    assert!(
        !test
            .engine
            .update_file_selection(HASH, &[], true)
            .await
            .unwrap()
    );
    assert!(
        test.engine
            .update_file_selection(HASH, &[0, 42, usize::MAX], true)
            .await
            .unwrap()
    );
    assert!(
        test.store
            .torrents()
            .get(added.torrent_id)
            .await
            .unwrap()
            .unwrap()
            .files()[0]
            .selected
    );
    test.engine
        .set_category(HASH, Some("category".into()))
        .await
        .unwrap();
    test.engine.set_priority(HASH, Some(3)).await.unwrap();
    let changed = test
        .store
        .torrents()
        .get(added.torrent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.category.as_deref(), Some("category"));
    assert_eq!(changed.priority, Some(3));
    test.engine.pause(HASH).await.unwrap();
    test.engine.resume(HASH).await.unwrap();
}

#[tokio::test]
async fn deletion_options_are_independent_and_retained_rows_are_terminal() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    let added = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    let _ = test.fake.take_calls();
    test.engine
        .delete(
            added.torrent_id,
            DeleteOptions {
                provider: true,
                ..DeleteOptions::default()
            },
        )
        .await
        .unwrap();
    let retained = test
        .store
        .torrents()
        .get(added.torrent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.error.as_deref(), Some("Torrent deleted"));
    assert!(retained.completed.is_some());
    assert!(
        test.fake
            .calls()
            .iter()
            .any(|call| matches!(call, ProviderCall::Delete { .. }))
    );
    test.engine
        .delete(
            added.torrent_id,
            DeleteOptions {
                data: true,
                ..DeleteOptions::default()
            },
        )
        .await
        .unwrap();
    assert!(
        test.store
            .torrents()
            .get(added.torrent_id)
            .await
            .unwrap()
            .is_none()
    );
    test.engine
        .delete(added.torrent_id, DeleteOptions::default())
        .await
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn cleanup_unlinks_final_symlinks_and_refuses_symlink_parents() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("precious"), "untouched")
        .await
        .unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("Movie")).unwrap();
    remove_local(root.path(), Path::new("Movie")).await.unwrap();
    assert!(outside.path().join("precious").exists());
    std::os::unix::fs::symlink(outside.path(), root.path().join("category")).unwrap();
    assert!(
        remove_local(root.path(), Path::new("category/precious"))
            .await
            .is_err()
    );
    assert!(
        remove_local(root.path(), Path::new("../precious"))
            .await
            .is_err()
    );
    assert!(outside.path().join("precious").exists());
}

#[tokio::test]
async fn waits_report_queue_timeout_missing_and_dequeued_without_provider_queries() {
    let mut settings = settings();
    settings.provider.api_key.clear();
    let test = TestEngine::with_settings(settings).await.unwrap();
    let added = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    assert_eq!(
        test.engine
            .wait_until_dequeued(added.torrent_id, Duration::from_millis(1))
            .await
            .unwrap(),
        WaitOutcome::TimedOut
    );
    assert_eq!(
        test.engine
            .wait_until_dequeued(Uuid::new_v4(), Duration::from_secs(1))
            .await
            .unwrap(),
        WaitOutcome::Missing
    );
    test.store
        .torrents()
        .update_rd_id(added.torrent_id, "assigned")
        .await
        .unwrap();
    assert_eq!(
        test.engine
            .wait_until_dequeued(added.torrent_id, Duration::from_secs(1))
            .await
            .unwrap(),
        WaitOutcome::Dequeued
    );
    assert_eq!(test.fake.calls(), Vec::new());
}

#[tokio::test]
async fn provider_add_failure_is_terminal_and_arms_only_one_automatic_retry() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    test.fake.queue_failure(
        ProviderOperation::AddMagnet,
        DebridError::Provider("rejected".into()),
    );
    let added = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    assert_eq!(
        added.error.as_deref(),
        Some("Could not add to provider: rejected")
    );
    assert!(added.completed.is_some());
    assert!(added.retry.is_some());
    assert_eq!(added.retry_count, 1);
    test.engine.dispatch_queued().await.unwrap();
    assert_eq!(
        test.fake
            .calls()
            .iter()
            .filter(|call| matches!(call, ProviderCall::AddMagnet(_)))
            .count(),
        1
    );
}

#[tokio::test]
async fn completed_and_shutdown_waits_end_without_provider_requests() {
    let mut settings = settings();
    settings.provider.api_key.clear();
    let test = TestEngine::with_settings(settings).await.unwrap();
    let added = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    test.store
        .torrents()
        .update_complete(
            added.torrent_id,
            Some("failed"),
            Some(Timestamp::now()),
            RetryOnError::Skip,
        )
        .await
        .unwrap();
    assert_eq!(
        test.engine
            .wait_until_dequeued(added.torrent_id, Duration::from_secs(1))
            .await
            .unwrap(),
        WaitOutcome::Completed
    );
    test.engine.inner.cancel.cancel();
    assert_eq!(
        test.engine
            .wait_until_dequeued(added.torrent_id, Duration::from_secs(1))
            .await
            .unwrap(),
        WaitOutcome::Shutdown
    );
    assert_eq!(test.fake.calls(), Vec::new());
}

#[tokio::test]
async fn delete_waits_for_extraction_to_finish_before_removing_rows_and_files() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = settings();
    settings.provider.api_key.clear();
    settings.download_client.download_path = directory.path().to_string_lossy().into_owned();
    let test = TestEngine::with_settings(settings).await.unwrap();
    let added = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    fs::create_dir(directory.path().join("Movie"))
        .await
        .unwrap();
    fs::write(directory.path().join("Movie/partial"), "partial")
        .await
        .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let (sender, completion) = rdc_download::completion_channel();
    let (_progress, progress) = tokio::sync::watch::channel(0_u8);
    test.engine.inner.registry.unpacks.lock().unwrap().insert(
        Uuid::new_v4(),
        Arc::new(crate::registry::ActiveUnpack {
            torrent_id: added.torrent_id,
            cancel: cancel.clone(),
            completion,
            progress,
        }),
    );
    let delete = test.engine.delete(
        added.torrent_id,
        DeleteOptions {
            data: true,
            provider: false,
            local_files: true,
        },
    );
    let finish = async {
        cancel.cancelled().await;
        assert!(
            test.store
                .torrents()
                .get(added.torrent_id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(directory.path().join("Movie/partial").exists());
        sender.complete(Ok(()));
    };
    let (deleted, ()) = time::timeout(Duration::from_secs(2), async {
        tokio::join!(delete, finish)
    })
    .await
    .unwrap();
    deleted.unwrap();
    assert!(
        test.store
            .torrents()
            .get(added.torrent_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!directory.path().join("Movie").exists());
}

#[tokio::test]
async fn availability_normalizes_hash_and_routes_provider_rate_limits() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    test.fake.set_available_files(
        HASH.into(),
        vec![AvailableFile {
            filename: "cached.mkv".into(),
            filesize: 42,
        }],
    );
    assert_eq!(
        test.engine
            .available_files(&HASH.to_uppercase())
            .await
            .unwrap()[0]
            .filename,
        "cached.mkv"
    );
    assert_eq!(test.engine.torrents().await.unwrap(), Vec::new());
    test.fake.queue_failure(
        ProviderOperation::AvailableFiles,
        DebridError::RateLimited {
            message: "slow down".into(),
            retry_after: Duration::from_secs(60),
        },
    );
    assert!(test.engine.available_files(HASH).await.is_err());
    assert!(
        test.engine
            .inner
            .coordinator
            .max_next_allowed_at()
            .is_some()
    );
}

#[tokio::test]
async fn duplicate_copy_uses_existing_name_and_stored_payload() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = settings();
    settings.provider.api_key.clear();
    settings.general.copy_added_torrents = Some(directory.path().to_string_lossy().into_owned());
    let test = TestEngine::with_settings(settings).await.unwrap();
    let added = test
        .engine
        .add_magnet(MAGNET, TorrentTemplate::default())
        .await
        .unwrap();
    fs::remove_file(directory.path().join("Movie.magnet"))
        .await
        .unwrap();
    let duplicate = test
        .engine
        .add_magnet(
            &format!("{MAGNET}&tr=udp%3A%2F%2Fdifferent.example"),
            TorrentTemplate::default(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.torrent_id, added.torrent_id);
    assert_eq!(
        fs::read_to_string(directory.path().join("Movie.magnet"))
            .await
            .unwrap(),
        MAGNET
    );
}

#[tokio::test]
async fn availability_parses_magnets_and_metadata_without_creating_rows() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    test.fake.set_available_files(
        HASH.into(),
        vec![AvailableFile {
            filename: "cached.mkv".into(),
            filesize: 42,
        }],
    );
    assert_eq!(
        test.engine
            .available_files_for_magnet(MAGNET)
            .await
            .unwrap()[0]
            .filename,
        "cached.mkv"
    );
    let bytes =
        b"d4:infod6:lengthi1e4:name5:Movie12:piece lengthi1e6:pieces20:abcdefghijklmnopqrstee";
    let hash = TorrentFile::parse(bytes).unwrap().info_hash_hex();
    test.fake.set_available_files(
        hash,
        vec![AvailableFile {
            filename: "metadata.mkv".into(),
            filesize: 7,
        }],
    );
    assert_eq!(
        test.engine
            .available_files_for_torrent(bytes)
            .await
            .unwrap()[0]
            .filename,
        "metadata.mkv"
    );
    assert_eq!(test.engine.torrents().await.unwrap(), Vec::new());
    assert!(!test.fake.calls().iter().any(|call| matches!(
        call,
        ProviderCall::AddMagnet(_) | ProviderCall::AddTorrentFile(_)
    )));
    assert!(
        test.engine
            .available_files_for_magnet("invalid")
            .await
            .is_err()
    );
    assert!(
        test.engine
            .available_files_for_torrent(b"invalid")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn parse_errors_name_the_magnet_but_not_the_torrent_bytes() {
    let test = TestEngine::with_settings(settings()).await.unwrap();
    let magnet_error = test
        .engine
        .add_magnet("magnet:?dn=missing", TorrentTemplate::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(magnet_error.ends_with(", trying to parse magnet:?dn=missing"));
    let file_error = test
        .engine
        .add_torrent_file(b"invalid", TorrentTemplate::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(file_error.ends_with(", trying to parse a 7-byte torrent file"));
    assert!(!file_error.contains(&BASE64.encode(b"invalid")));
    let availability_error = test
        .engine
        .available_files_for_torrent(b"invalid")
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(availability_error, file_error);
    test.engine.pause("missing").await.unwrap();
    test.engine.resume("missing").await.unwrap();
}

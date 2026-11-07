use super::tests::{HASH, add, current, finish, fixture, template};
use super::*;
use crate::{
    DeleteOptions, EngineOptions, TorrentTemplate,
    testing::{ProviderCall, TestEngine, downloads::FakeDownloadFactory},
};
use rdc_model::TorrentFinishedAction;
use sqlx::{Connection, sqlite::SqliteConnectOptions};
use std::path::PathBuf;
use tokio::fs;

fn minutes_ago(minutes: i64) -> Timestamp {
    Timestamp::now()
        .checked_sub(SignedDuration::from_mins(minutes))
        .expect("past instant")
}

fn torrent_folder(test: &TestEngine) -> PathBuf {
    PathBuf::from(&test.engine.settings().download_client.download_path).join("Example")
}

fn provider_deletes(test: &TestEngine) -> usize {
    test.fake
        .calls()
        .iter()
        .filter(|call| matches!(call, ProviderCall::Delete { .. }))
        .count()
}

async fn mark_completed(test: &TestEngine, id: Uuid, error: Option<&str>, at: Timestamp) {
    test.store
        .torrents()
        .update_complete(id, error, Some(at), RetryOnError::Skip)
        .await
        .expect("completion timestamp");
}

/// Moves the stored addition time into the past; the store never rewrites it.
async fn backdate_added(test: &TestEngine, id: Uuid, at: Timestamp) {
    let options = SqliteConnectOptions::new().filename(test.tempdir.path().join("rd-client.db"));
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .expect("database connection");
    sqlx::query(r#"UPDATE "Torrents" SET "Added" = ? WHERE "TorrentId" = ?"#)
        .bind(at.strftime("%Y-%m-%d %H:%M:%S+00:00").to_string())
        .bind(id.hyphenated().to_string().to_uppercase())
        .execute(&mut connection)
        .await
        .expect("backdated addition");
}

async fn wait_until<F: Fn() -> bool>(condition: F) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition reached in time");
}

#[tokio::test]
async fn expired_delete_on_error_removes_the_torrent_with_provider_and_local_data() {
    let (test, _) = fixture().await;
    let torrent = add(
        &test,
        HASH,
        "one",
        &["a.bin"],
        TorrentTemplate {
            delete_on_error: 10,
            ..template()
        },
    )
    .await;
    test.fake.set_selection("one".to_owned(), Some(0));
    test.engine.tick_once().await.expect("selection");
    assert_eq!(
        current(&test, torrent.torrent_id).await.error.as_deref(),
        Some("All files excluded")
    );
    let folder = torrent_folder(&test);
    fs::create_dir_all(&folder).await.expect("folder");
    fs::write(folder.join("leftover.bin"), b"partial")
        .await
        .expect("leftover");
    test.engine.tick_once().await.expect("retention pending");
    assert!(folder.exists());
    mark_completed(
        &test,
        torrent.torrent_id,
        Some("All files excluded"),
        minutes_ago(11),
    )
    .await;
    test.engine.tick_once().await.expect("retention expired");
    assert!(
        test.store
            .torrents()
            .get(torrent.torrent_id)
            .await
            .expect("read")
            .is_none()
    );
    assert!(!folder.exists());
    assert_eq!(provider_deletes(&test), 1);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn retrying_an_imported_torrent_without_payload_deletes_nothing() {
    let (test, _) = fixture().await;
    let template = Torrent {
        rd_name: Some("Example".to_owned()),
        torrent_retry_attempts: 2,
        ..Torrent::default()
    };
    let torrent = test
        .store
        .torrents()
        .add(&rdc_store::NewTorrent {
            rd_id: Some("one"),
            hash: HASH,
            payload: None,
            is_file: false,
            download_client: DownloadClient::Internal,
            template: &template,
        })
        .await
        .expect("imported torrent");
    let folder = torrent_folder(&test);
    fs::create_dir_all(&folder).await.expect("folder");
    fs::write(folder.join("done.bin"), b"done")
        .await
        .expect("downloaded file");
    test.store
        .torrents()
        .update_complete(
            torrent.torrent_id,
            Some("Runner error: boom"),
            Some(Timestamp::now()),
            RetryOnError::Schedule,
        )
        .await
        .expect("failure schedules a retry");
    test.engine.tick_once().await.expect("retry tick");
    let kept = current(&test, torrent.torrent_id).await;
    assert_eq!(kept.retry, None);
    assert_eq!(kept.rd_id.as_deref(), Some("one"));
    assert_eq!(
        kept.error.as_deref(),
        Some("Cannot re-add this torrent: stored magnet or file not found")
    );
    assert!(folder.join("done.bin").exists());
    assert_eq!(provider_deletes(&test), 0);
    let manual = test.engine.retry(torrent.torrent_id).await;
    assert!(manual.is_err());
    assert_eq!(current(&test, torrent.torrent_id).await.retry, None);
    assert!(folder.join("done.bin").exists());
    assert_eq!(provider_deletes(&test), 0);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn lifetime_expiry_fails_torrents_without_downloads_and_disables_retries() {
    let (test, _) = fixture().await;
    test.fake
        .set_status("one".to_owned(), TorrentStatus::Downloading);
    let torrent = add(
        &test,
        HASH,
        "one",
        &["a.bin"],
        TorrentTemplate {
            lifetime: 5,
            torrent_retry_attempts: 2,
            ..template()
        },
    )
    .await;
    test.engine.tick_once().await.expect("within lifetime");
    assert!(current(&test, torrent.torrent_id).await.completed.is_none());
    backdate_added(&test, torrent.torrent_id, minutes_ago(6)).await;
    test.engine.tick_once().await.expect("lifetime reached");
    let expired = current(&test, torrent.torrent_id).await;
    assert_eq!(
        expired.error.as_deref(),
        Some("Torrent lifetime of 5 minutes reached")
    );
    assert!(expired.completed.is_some());
    assert!(expired.retry.is_none());
    assert_eq!(expired.retry_count, 2);
    test.engine.tick_once().await.expect("no retry");
    assert_eq!(
        test.engine.torrents().await.expect("list")[0].torrent_id,
        torrent.torrent_id
    );
    test.engine.shutdown().await;
}

#[tokio::test]
async fn finished_action_waits_for_its_delay_and_symlink_torrents_keep_provider_content() {
    let (test, factory) = fixture().await;
    let torrent = add(
        &test,
        HASH,
        "one",
        &["file.bin"],
        TorrentTemplate {
            finished_action: TorrentFinishedAction::RemoveAllTorrents,
            finished_action_delay: 30,
            download_client: DownloadClient::Symlink,
            ..template()
        },
    )
    .await;
    test.engine.tick_once().await.expect("create");
    test.engine.tick_once().await.expect("start");
    finish(&test, &factory, 0, b"linked").await;
    test.engine.tick_once().await.expect("complete");
    let completed = current(&test, torrent.torrent_id).await;
    assert!(completed.completed.is_some());
    assert!(completed.error.is_none());
    test.engine.tick_once().await.expect("delay pending");
    assert!(current(&test, torrent.torrent_id).await.completed.is_some());
    mark_completed(&test, torrent.torrent_id, None, minutes_ago(31)).await;
    test.engine.tick_once().await.expect("delay elapsed");
    assert!(
        test.store
            .torrents()
            .get(torrent.torrent_id)
            .await
            .expect("read")
            .is_none()
    );
    assert_eq!(provider_deletes(&test), 0);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn provider_only_removal_runs_once_and_keeps_a_successful_torrent_clean() {
    let (test, factory) = fixture().await;
    let torrent = add(
        &test,
        HASH,
        "one",
        &["file.bin"],
        TorrentTemplate {
            finished_action: TorrentFinishedAction::RemoveProvider,
            delete_on_error: 1,
            ..template()
        },
    )
    .await;
    test.engine.tick_once().await.expect("create");
    test.engine.tick_once().await.expect("start");
    finish(&test, &factory, 0, b"complete").await;
    test.engine.tick_once().await.expect("complete");
    test.engine.tick_once().await.expect("finished action");
    // Past the delete-on-error delay, an errored row would now lose its files.
    mark_completed(&test, torrent.torrent_id, None, minutes_ago(2)).await;
    test.engine.tick_once().await.expect("not repeated");
    let kept = current(&test, torrent.torrent_id).await;
    assert_eq!(kept.error, None);
    assert!(kept.completed.is_some());
    assert_eq!(kept.rd_id, None);
    assert_eq!(provider_deletes(&test), 1);
    assert!(factory.requests()[0].local_path.exists());
    test.engine.shutdown().await;
}

#[tokio::test]
async fn provider_error_status_fails_the_torrent_and_arms_a_retry() {
    let (test, _) = fixture().await;
    test.fake.set_status("one".to_owned(), TorrentStatus::Error);
    let torrent = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.engine.tick_once().await.expect("provider error");
    let failed = current(&test, torrent.torrent_id).await;
    let error = failed.error.expect("provider error message");
    assert!(error.starts_with("Debrid error: "), "{error}");
    assert!(error.ends_with('.'), "{error}");
    assert!(failed.completed.is_some());
    assert!(failed.retry.is_some());
    assert_eq!(failed.retry_count, 1);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn deleting_a_torrent_cancels_its_active_transfer_and_nothing_restarts() {
    let (test, factory) = fixture().await;
    let torrent = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.engine.tick_once().await.expect("create");
    test.engine.tick_once().await.expect("start");
    assert_eq!(factory.engines().len(), 1);
    test.engine
        .delete(
            torrent.torrent_id,
            DeleteOptions {
                data: true,
                provider: true,
                local_files: true,
            },
        )
        .await
        .expect("delete");
    assert_eq!(factory.engines()[0].cancelled(), 1);
    assert!(
        test.engine
            .inner
            .registry
            .downloads
            .lock()
            .expect("registry")
            .is_empty()
    );
    test.engine.tick_once().await.expect("after delete");
    assert_eq!(factory.engines().len(), 1);
    assert_eq!(test.engine.torrents().await.expect("list"), Vec::new());
    test.engine.shutdown().await;
}

#[tokio::test]
async fn restart_requeues_an_interrupted_transfer_and_adopts_its_remote_identity() {
    let (test, factory) = fixture().await;
    let torrent = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.engine.tick_once().await.expect("create");
    factory.push({
        let engine = crate::testing::downloads::FakeDownloadEngine::new();
        engine.set_start_result(Ok("remote-7".to_owned()));
        engine
    });
    test.engine.tick_once().await.expect("start");
    let started = current(&test, torrent.torrent_id).await;
    assert!(started.downloads[0].download_started.is_some());
    assert_eq!(started.downloads[0].remote_id.as_deref(), Some("remote-7"));
    test.engine.shutdown().await;

    let restarted_factory = FakeDownloadFactory::new();
    let options = EngineOptions {
        download_factory: Some(Arc::new(restarted_factory.clone())),
        ..test.fake.options()
    };
    let engine = Engine::start(test.store.clone(), options)
        .await
        .expect("restart");
    let repaired = current(&test, torrent.torrent_id).await;
    assert!(repaired.downloads[0].download_started.is_none());
    engine.tick_once().await.expect("resume");
    let requests = restarted_factory.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].download.remote_id.as_deref(), Some("remote-7"));
    engine.shutdown().await;
}

#[tokio::test]
async fn restart_finishes_transitions_that_were_stored_only_in_part() {
    let (test, _) = fixture().await;
    let mut ids = Vec::new();
    for (hash, id) in [
        ("2123456789012345678901234567890123456789", "queued"),
        ("3123456789012345678901234567890123456789", "failed"),
        ("4123456789012345678901234567890123456789", "unpacked"),
    ] {
        ids.push(
            add(&test, hash, id, &["a.bin"], template())
                .await
                .torrent_id,
        );
    }
    test.engine.tick_once().await.expect("create");
    test.engine.shutdown().await;
    let downloads = test.store.downloads();
    let now = Some(Timestamp::now());
    let mut download_ids = Vec::new();
    for id in &ids {
        let download = current(&test, *id).await.downloads[0].download_id;
        downloads
            .update_link(download, "https://example.invalid/a.bin")
            .await
            .expect("link");
        downloads
            .update_download_started(download, now)
            .await
            .expect("started");
        download_ids.push(download);
    }
    // A finished transfer whose unpacking was never queued.
    downloads
        .update_download_finished(download_ids[0], now)
        .await
        .expect("finished");
    // A failure whose completion was never stored.
    downloads
        .update_error(download_ids[1], Some("transfer failed"))
        .await
        .expect("error");
    // A finished extraction whose completion was never stored.
    let unpacked = download_ids[2];
    downloads
        .update_download_finished(unpacked, now)
        .await
        .expect("finished");
    downloads
        .update_unpacking_queued(unpacked, now)
        .await
        .expect("unpack queued");
    downloads
        .update_unpacking_started(unpacked, now)
        .await
        .expect("unpack started");
    downloads
        .update_unpacking_finished(unpacked, now)
        .await
        .expect("unpacked");

    let engine = Engine::start(test.store.clone(), test.fake.options())
        .await
        .expect("restart");
    engine.tick_once().await.expect("tick");
    for (index, id) in ids.iter().enumerate() {
        let torrent = current(&test, *id).await;
        assert!(torrent.completed.is_some(), "{index}");
        assert!(torrent.downloads[0].completed.is_some(), "{index}");
        assert_eq!(torrent.error.is_some(), index == 1, "{index}");
    }
    engine.shutdown().await;
}

#[tokio::test]
async fn background_tick_loop_advances_torrents_and_stops_on_shutdown() {
    let (test, factory) = fixture().await;
    let torrent = add(&test, HASH, "one", &["a.bin"], template()).await;
    let runner = tokio::spawn(crate::background::tick_loop(test.engine.clone()));
    // The engine is started only after it is registered and its start is persisted.
    wait_until(|| {
        factory
            .engines()
            .first()
            .is_some_and(|engine| engine.started() == 1)
    })
    .await;
    // The loop owns completion collection, so the transfer is finished through its backend
    // without inspecting the registry the loop is concurrently draining.
    let request = factory.requests()[0].clone();
    fs::write(&request.local_path, b"complete")
        .await
        .expect("download file");
    factory.engines()[0].complete(Ok(()));
    let store = test.store.clone();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let row = store
                .torrents()
                .get(torrent.torrent_id)
                .await
                .expect("read")
                .expect("torrent");
            if row.completed.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("completed by the background loop");
    test.engine.shutdown().await;
    tokio::time::timeout(Duration::from_secs(5), runner)
        .await
        .expect("loop stops")
        .expect("loop task");
}

#[cfg(unix)]
#[tokio::test]
async fn completion_runs_the_configured_program_without_blocking_the_tick() {
    let (test, factory) = fixture().await;
    test.engine
        .update_settings(vec![
            (
                "General:RunOnTorrentCompleteFileName".to_owned(),
                Some("/usr/bin/touch".to_owned()),
            ),
            (
                "General:RunOnTorrentCompleteArguments".to_owned(),
                Some("%R/completion-marker".to_owned()),
            ),
        ])
        .await
        .expect("program settings");
    let torrent = add(&test, HASH, "one", &["a.bin"], template()).await;
    test.engine.tick_once().await.expect("create");
    test.engine.tick_once().await.expect("start");
    finish(&test, &factory, 0, b"complete").await;
    test.engine.tick_once().await.expect("complete");
    assert!(current(&test, torrent.torrent_id).await.completed.is_some());
    let marker = PathBuf::from(&test.engine.settings().download_client.download_path)
        .join("completion-marker");
    wait_until(|| marker.exists()).await;
    test.engine.shutdown().await;
}

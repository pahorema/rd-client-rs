//! Repairs interrupted host work without discarding remote transfer identities.

use crate::EngineError;
use jiff::Timestamp;
use rdc_model::settings::Settings;
use rdc_store::Store;

pub(crate) async fn repair(store: &Store, settings: &Settings) -> Result<(), EngineError> {
    tracing::debug!(?settings, "loaded engine settings");
    let now = Some(Timestamp::now());
    for torrent in store.torrents().list_incomplete().await? {
        for download in torrent.downloads {
            if download.completed.is_some() {
                continue;
            }
            // Finish transitions that older versions or a crash stored only in part.
            if download.error.is_some() || download.unpacking_finished.is_some() {
                store
                    .downloads()
                    .update_completed(download.download_id, now)
                    .await?;
                continue;
            }
            if download.download_finished.is_some() && download.unpacking_queued.is_none() {
                store
                    .downloads()
                    .update_unpacking_queued(download.download_id, now)
                    .await?;
                continue;
            }
            if download.download_queued.is_some()
                && download.download_started.is_some()
                && download.download_finished.is_none()
            {
                store
                    .downloads()
                    .update_download_started(download.download_id, None)
                    .await?;
            }
            if download.unpacking_queued.is_some()
                && download.unpacking_started.is_some()
                && download.unpacking_finished.is_none()
            {
                store
                    .downloads()
                    .update_unpacking_started(download.download_id, None)
                    .await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, testing::FakeDebridProvider};
    use rdc_model::{DownloadInfo, Torrent};
    use rdc_store::{NewTorrent, RetryOnError};

    async fn interrupted(
        store: &Store,
        finished: bool,
        failed: bool,
        completed_torrent: bool,
    ) -> uuid::Uuid {
        let template = Torrent::default();
        let torrent = store
            .torrents()
            .add(&NewTorrent {
                rd_id: Some("remote"),
                hash: &uuid::Uuid::new_v4().to_string(),
                payload: None,
                is_file: false,
                download_client: template.download_client,
                template: &template,
            })
            .await
            .expect("torrent");
        store
            .downloads()
            .try_add(
                torrent.torrent_id,
                &DownloadInfo {
                    file_name: Some("file.zip".to_owned()),
                    restricted_link: "https://example.invalid/file".to_owned(),
                },
            )
            .await
            .expect("download");
        let id = store
            .downloads()
            .for_torrent(torrent.torrent_id)
            .await
            .expect("downloads")[0]
            .download_id;
        let now = Some(Timestamp::now());
        store
            .downloads()
            .update_download_started(id, now)
            .await
            .expect("started");
        store
            .downloads()
            .update_unpacking_queued(id, now)
            .await
            .expect("unpack queued");
        store
            .downloads()
            .update_unpacking_started(id, now)
            .await
            .expect("unpack started");
        store
            .downloads()
            .update_remote_id(id, "adopt-me")
            .await
            .expect("remote id");
        if finished {
            store
                .downloads()
                .update_download_finished(id, now)
                .await
                .expect("finished");
            store
                .downloads()
                .update_unpacking_finished(id, now)
                .await
                .expect("unpacked");
        }
        if failed {
            store
                .downloads()
                .update_error(id, Some("failed"))
                .await
                .expect("error");
        }
        if completed_torrent {
            store
                .torrents()
                .update_complete(torrent.torrent_id, None, now, RetryOnError::Skip)
                .await
                .expect("completed");
        }
        id
    }

    #[tokio::test]
    async fn startup_requeues_interrupted_work_but_retains_remote_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("db.sqlite"))
            .await
            .expect("store");
        let interrupted = interrupted(&store, false, false, false).await;
        let fake = FakeDebridProvider::default();
        let engine = Engine::start(store.clone(), fake.options())
            .await
            .expect("engine");
        let download = store
            .downloads()
            .get(interrupted)
            .await
            .expect("read")
            .expect("download")
            .download;
        assert!(download.download_started.is_none());
        assert!(download.unpacking_started.is_none());
        assert!(download.download_queued.is_some());
        assert!(download.unpacking_queued.is_some());
        assert_eq!(download.remote_id.as_deref(), Some("adopt-me"));
        assert_eq!(fake.calls(), Vec::new());
        engine.shutdown().await;
    }

    #[tokio::test]
    async fn startup_completes_finished_or_failed_work_without_reopening_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("db.sqlite"))
            .await
            .expect("store");
        let mut ids = Vec::new();
        for flags in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            ids.push(interrupted(&store, flags.0, flags.1, flags.2).await);
        }
        let engine = Engine::start(store.clone(), FakeDebridProvider::default().options())
            .await
            .expect("engine");
        for (index, id) in ids.into_iter().enumerate() {
            let download = store
                .downloads()
                .get(id)
                .await
                .expect("read")
                .expect("download")
                .download;
            assert!(download.download_started.is_some());
            assert!(download.unpacking_started.is_some());
            // Work of a completed torrent is left alone.
            assert_eq!(download.completed.is_some(), index < 2, "{index}");
        }
        engine.shutdown().await;
    }
}

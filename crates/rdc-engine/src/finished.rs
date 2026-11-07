//! Torrent completion notification and delayed provider/client cleanup.

use crate::{
    DeleteOptions, Engine, EngineError,
    completion::{self, CompletionSettings},
    tick::minutes_elapsed,
};
use rdc_model::{
    DownloadClient, Torrent, TorrentFinishedAction, TorrentHostDownloadAction, TorrentStatus,
};
use rdc_store::RetryOnError;

impl Engine {
    pub(crate) async fn complete_if_ready(&self, torrent: &Torrent) -> Result<(), EngineError> {
        if torrent.completed.is_some() {
            return Ok(());
        }
        let eligible = !torrent.downloads.is_empty()
            || (torrent.rd_status == Some(TorrentStatus::Finished)
                && torrent.host_download_action == TorrentHostDownloadAction::DownloadNone);
        if !eligible
            || torrent
                .downloads
                .iter()
                .any(|download| download.completed.is_none())
        {
            return Ok(());
        }
        self.inner
            .store
            .torrents()
            .update_complete(
                torrent.torrent_id,
                None,
                Some(jiff::Timestamp::now()),
                RetryOnError::Schedule,
            )
            .await?;
        let settings = self.settings();
        let program = CompletionSettings {
            program: settings.general.run_on_torrent_complete_file_name.clone(),
            arguments: settings.general.run_on_torrent_complete_arguments.clone(),
            download_path: settings.download_client.download_path.clone().into(),
        };
        if let Some(job) =
            completion::run_torrent_complete(&program, torrent, torrent.downloads.len())
        {
            let mut jobs = self
                .inner
                .completion_jobs
                .lock()
                .expect("completion jobs lock poisoned");
            jobs.retain(|job| !job.is_finished());
            jobs.push(job);
        }
        Ok(())
    }

    pub(crate) async fn process_finished_actions(
        &self,
        torrents: &[Torrent],
    ) -> Result<(), EngineError> {
        for torrent in torrents {
            if torrent.error.is_some()
                || !torrent
                    .completed
                    .is_some_and(|at| minutes_elapsed(at, torrent.finished_action_delay))
            {
                continue;
            }
            let action = match (torrent.download_client, torrent.finished_action) {
                (DownloadClient::Symlink, TorrentFinishedAction::RemoveAllTorrents) => {
                    TorrentFinishedAction::RemoveClient
                }
                (DownloadClient::Symlink, TorrentFinishedAction::RemoveProvider) => {
                    TorrentFinishedAction::None
                }
                (_, action) => action,
            };
            let options = match action {
                TorrentFinishedAction::None => continue,
                TorrentFinishedAction::RemoveAllTorrents => DeleteOptions {
                    data: true,
                    provider: true,
                    local_files: false,
                },
                // A kept row without provider id has had its provider torrent removed.
                TorrentFinishedAction::RemoveProvider if torrent.rd_id.is_none() => continue,
                TorrentFinishedAction::RemoveProvider => DeleteOptions {
                    data: false,
                    provider: true,
                    local_files: false,
                },
                TorrentFinishedAction::RemoveClient => DeleteOptions {
                    data: true,
                    provider: false,
                    local_files: false,
                },
            };
            // Provider removal waits for a cooldown instead of failing and orphaning the torrent.
            if options.provider
                && torrent.rd_id.is_some()
                && self.inner.coordinator.max_next_allowed_at().is_some()
            {
                continue;
            }
            self.delete(torrent.torrent_id, options).await?;
        }
        Ok(())
    }
}

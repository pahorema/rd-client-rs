//! Archive scheduling, multi-volume readiness and extraction result persistence.

use crate::{Engine, EngineError, download_runtime::output_location, registry::ActiveUnpack};
use jiff::Timestamp;
use rdc_archive::UnpackRequest;
use rdc_model::{Download, DownloadClient, Provider, Torrent};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::watch;

impl Engine {
    pub(crate) async fn collect_completed_unpacks(&self) -> Result<(), EngineError> {
        let finished: Vec<_> = self
            .inner
            .registry
            .unpacks
            .lock()
            .expect("unpack registry lock poisoned")
            .iter()
            .filter_map(|(id, active)| {
                active
                    .completion
                    .outcome()
                    .map(|outcome| (*id, active.clone(), outcome))
            })
            .collect();
        for (id, active, outcome) in finished {
            let _guard = self.inner.provider_lock.lock().await;
            let registered = self
                .inner
                .registry
                .unpacks
                .lock()
                .expect("unpack registry lock poisoned")
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &active));
            if !registered {
                continue;
            }
            // A transition of a download deleted meanwhile changes nothing.
            match outcome {
                Ok(()) => {
                    self.inner
                        .store
                        .downloads()
                        .mark_unpacked(id, Timestamp::now())
                        .await?;
                }
                Err(error) => {
                    self.inner
                        .store
                        .downloads()
                        .mark_failed(id, &error, Timestamp::now())
                        .await?;
                }
            }
            self.inner
                .registry
                .unpacks
                .lock()
                .expect("unpack registry lock poisoned")
                .remove(&id);
        }
        Ok(())
    }

    pub(crate) async fn start_unpacks(
        &self,
        torrent: &Torrent,
        limit: usize,
    ) -> Result<(), EngineError> {
        if !torrent.downloads.iter().any(awaits_unpack) {
            return Ok(());
        }
        let settings = self.settings();
        let root = Path::new(&settings.download_client.download_path);
        let files = torrent.files();
        let mut groups: HashMap<PathBuf, Vec<&Download>> = HashMap::new();
        let mut unknown_pending = false;
        let mut unknown_failed = false;
        for download in &torrent.downloads {
            if awaits_unpack(download) {
                let Some(link) = download.link.as_deref() else {
                    self.fail_unpack(download.download_id, "Download Link cannot be null")
                        .await?;
                    unknown_failed = true;
                    continue;
                };
                url::Url::parse(link).map_err(|error| EngineError::Message(error.to_string()))?;
            }
            if download
                .file_name
                .as_deref()
                .is_none_or(|name| name.trim().is_empty())
                && download.link.is_none()
            {
                unknown_failed |= download.error.is_some();
                unknown_pending |=
                    download.download_finished.is_none() && download.completed.is_none();
                continue;
            }
            // File names supplied with provider links are sufficient to identify an unfinished volume.
            let identity = Download {
                link: download
                    .link
                    .clone()
                    .or_else(|| Some(download.path.clone())),
                file_name: download.file_name.clone(),
                ..Download::default()
            };
            // Only the group that starts needs its directories; see `output_paths` below.
            let (_, output, _) = output_location(root, torrent, &files, &identity)?;
            groups
                .entry(rdc_archive::first_volume(&output))
                .or_default()
                .push(download);
        }
        // Iterate in queue order, independent of hash-map iteration order.
        let mut groups: Vec<_> = groups.into_iter().collect();
        groups.sort_by_key(|(_, downloads)| {
            downloads
                .iter()
                .filter_map(|download| download.download_queued)
                .min()
        });
        for (archive, downloads) in groups {
            let queued: Vec<_> = downloads
                .iter()
                .copied()
                .filter(|download| awaits_unpack(download))
                .collect();
            if queued.is_empty() {
                continue;
            }
            if limit == 0
                || torrent.download_client == DownloadClient::Symlink
                || !archive_trigger(&archive)
            {
                for download in queued {
                    self.skip_unpack(download.download_id).await?;
                }
                continue;
            }
            if unknown_failed
                || downloads.iter().any(|download| {
                    download.error.is_some() && download.download_finished.is_none()
                })
            {
                for download in queued {
                    self.fail_unpack(
                        download.download_id,
                        "An archive volume did not finish downloading",
                    )
                    .await?;
                }
                continue;
            }
            if unknown_pending
                || downloads
                    .iter()
                    .any(|download| download.download_finished.is_none())
            {
                continue;
            }
            let active = {
                let active = self
                    .inner
                    .registry
                    .unpacks
                    .lock()
                    .expect("unpack registry lock poisoned");
                if downloads
                    .iter()
                    .any(|download| active.contains_key(&download.download_id))
                {
                    continue;
                }
                active
                    .values()
                    .map(Arc::as_ptr)
                    .collect::<HashSet<_>>()
                    .len()
            };
            if active >= limit {
                continue;
            }
            let primary = queued[0];
            let (destination, _, _) = self.output_paths(torrent, primary).await?;
            let name = torrent
                .rd_name
                .as_deref()
                .map(rdc_model::paths::remove_invalid_file_name_chars)
                .filter(|name| !name.trim().is_empty() && name != ".")
                .ok_or_else(|| EngineError::Message("Invalid download path".to_owned()))?;
            let request = UnpackRequest {
                archive,
                destination,
                torrent_name: name,
                flatten_hash_dir: (torrent.client_kind == Some(Provider::TorBox))
                    .then(|| torrent.hash.clone()),
            };
            let _guard = self.inner.provider_lock.lock().await;
            let mut valid = true;
            for download in &queued {
                let Some(current) = self
                    .inner
                    .store
                    .downloads()
                    .get(download.download_id)
                    .await?
                else {
                    valid = false;
                    break;
                };
                if current.torrent.completed.is_some() || current.download != **download {
                    valid = false;
                    break;
                }
            }
            if !valid {
                continue;
            }
            let cancel = self.inner.cancel.child_token();
            let (progress, receiver) = watch::channel(0_u8);
            let (completion, outcome) = rdc_download::completion_channel();
            let active = Arc::new(ActiveUnpack {
                torrent_id: torrent.torrent_id,
                cancel: cancel.clone(),
                completion: outcome,
                progress: receiver,
            });
            for download in &queued {
                self.inner
                    .store
                    .downloads()
                    .update_unpacking_started(download.download_id, Some(Timestamp::now()))
                    .await?;
            }
            {
                let mut registry = self
                    .inner
                    .registry
                    .unpacks
                    .lock()
                    .expect("unpack registry lock poisoned");
                for download in queued {
                    registry.insert(download.download_id, active.clone());
                }
            }
            let link = primary.link.clone().unwrap_or_default();
            let torrent_name = torrent.rd_name.clone().unwrap_or_default();
            tokio::spawn(async move {
                let result = rdc_archive::unpack(
                    request,
                    move |value| {
                        progress.send_replace(value);
                    },
                    cancel,
                )
                .await
                .map_err(|error| error.unpack_failure_message(Some(&link), &torrent_name));
                completion.complete(result);
            });
        }
        Ok(())
    }

    async fn fail_unpack(&self, id: uuid::Uuid, error: &str) -> Result<(), EngineError> {
        self.inner
            .store
            .downloads()
            .mark_failed(id, error, Timestamp::now())
            .await?;
        Ok(())
    }

    async fn skip_unpack(&self, id: uuid::Uuid) -> Result<(), EngineError> {
        self.inner
            .store
            .downloads()
            .mark_unpacked(id, Timestamp::now())
            .await?;
        Ok(())
    }
}

/// Whether the download is queued for unpacking and its unpacking has not started.
fn awaits_unpack(download: &Download) -> bool {
    download.completed.is_none()
        && download.unpacking_queued.is_some()
        && download.unpacking_started.is_none()
        && download.error.is_none()
}

fn archive_trigger(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "rar" || extension == "zip" || extension == "001")
}

#[cfg(test)]
#[path = "unpack_runtime_tests.rs"]
mod tests;

//! Helpers shared by the integration tests.

use std::path::PathBuf;

use rdc_model::{DownloadClient, Torrent};
use rdc_store::{NewTorrent, Store};
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use tempfile::TempDir;

/// A store on a new database, with the file path for direct inspection.
pub(crate) struct TestStore {
    pub(crate) store: Store,
    path: PathBuf,
    _dir: TempDir,
}

impl TestStore {
    pub(crate) async fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("rd-client.db");
        let store = Store::open(&path).await.expect("open store");
        Self {
            store,
            path,
            _dir: dir,
        }
    }

    /// A second, independent connection pool on the same file.
    pub(crate) async fn raw(&self) -> SqlitePool {
        SqlitePool::connect_with(SqliteConnectOptions::new().filename(&self.path))
            .await
            .expect("raw pool")
    }

    /// Adds a torrent with default options and the given hash.
    pub(crate) async fn add_torrent(&self, hash: &str) -> Torrent {
        let template = Torrent::default();
        self.store
            .torrents()
            .add(&NewTorrent {
                rd_id: None,
                hash,
                payload: None,
                is_file: false,
                download_client: DownloadClient::Internal,
                template: &template,
            })
            .await
            .expect("add torrent")
    }
}

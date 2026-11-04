//! SQLite persistence for rd-client: torrents, downloads, settings and the
//! user account, stored in the existing database layout.

mod codec;
mod downloads;
mod error;
mod row;
mod schema;
#[cfg(test)]
mod test_support;
mod torrents;

use std::{path::Path, time::Duration};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

pub use error::{Result, StoreError};
pub use torrents::{NewTorrent, RetryOnError, Torrents};

/// How long a connection waits for a lock held by another connection.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Page cache size per connection; negative values are KiB (about 64 MB).
const CACHE_SIZE_KIB: &str = "-64000";

/// Handle to the database; cheap to clone, every clone shares one pool.
#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Opens the database at `path`, creating its directory and the file
    /// when they are missing, and brings it to the current layout.
    ///
    /// An empty file receives the current layout; a database with a
    /// migration history gets every missing migration applied in order.
    ///
    /// Every pooled connection enforces foreign keys and uses write-ahead
    /// logging, `NORMAL` synchronisation, a five second busy timeout and a
    /// 64 MB page cache.
    ///
    /// # Errors
    ///
    /// Fails when the directory cannot be created, the database cannot be
    /// opened, it has tables but no migration history
    /// ([`StoreError::UnversionedDatabase`]), it was upgraded by a newer
    /// version ([`StoreError::NewerDatabase`]) or a migration fails
    /// ([`StoreError::Migration`]).
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent).await.map_err(|source| {
                StoreError::CreateDirectory {
                    path: parent.to_owned(),
                    source,
                }
            })?;
        }
        let options = connect_options(path);
        schema::prepare(&options).await?;
        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        Ok(Self { pool })
    }

    /// Torrent operations.
    pub fn torrents(&self) -> Torrents<'_> {
        Torrents::new(&self.pool)
    }

    /// Waits for running operations and closes every connection.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

fn connect_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT)
        .pragma("cache_size", CACHE_SIZE_KIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pragma(connection: &mut sqlx::SqliteConnection, name: &str) -> String {
        let sql = format!("SELECT CAST((SELECT * FROM pragma_{name}) AS TEXT)");
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .fetch_one(connection)
            .await
            .expect("pragma is readable")
    }

    #[tokio::test]
    async fn open_creates_missing_directories_and_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested/db/rd-client.db");
        let store = Store::open(&path).await.expect("open");
        store.close().await;
        assert!(path.is_file());
    }

    #[tokio::test]
    async fn every_pooled_connection_gets_the_pragmas() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open(dir.path().join("rd-client.db"))
            .await
            .expect("open");
        let mut first = store.pool.acquire().await.expect("first connection");
        let mut second = store.pool.acquire().await.expect("second connection");
        for connection in [&mut *first, &mut *second] {
            assert_eq!(pragma(connection, "foreign_keys").await, "1");
            assert_eq!(pragma(connection, "journal_mode").await, "wal");
            assert_eq!(pragma(connection, "synchronous").await, "1");
            assert_eq!(pragma(connection, "busy_timeout").await, "5000");
            assert_eq!(pragma(connection, "cache_size").await, "-64000");
        }
    }
}

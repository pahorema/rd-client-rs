//! Schema management: brings a database file to the current layout.
//!
//! The layout is versioned by a migration history table listing every
//! applied migration. A missing migration is applied in its own write
//! transaction together with its history row, so an interrupted upgrade
//! leaves the database at the previous migration.

mod migrations;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use sqlx::{ConnectOptions, Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tracing::info;

use crate::error::{Result, StoreError};

use self::migrations::{MIGRATIONS, Migration};

/// Migration history table of the existing database layout.
const HISTORY_TABLE: &str = "__EFMigrationsHistory";

/// Version recorded next to every migration applied by this crate.
const PRODUCT_VERSION: &str = "10.0.5";

const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS "__EFMigrationsHistory" (
    "MigrationId" TEXT NOT NULL CONSTRAINT "PK___EFMigrationsHistory" PRIMARY KEY,
    "ProductVersion" TEXT NOT NULL
)"#;

/// Brings the database behind `options` to the current layout.
///
/// Runs on a dedicated connection with foreign keys disabled, because some
/// migrations rebuild tables that other tables reference.
pub(crate) async fn prepare(options: &SqliteConnectOptions) -> Result<()> {
    let mut connection = options.clone().foreign_keys(false).connect().await?;
    let result = upgrade(&mut connection, MIGRATIONS).await;
    connection.close().await?;
    result
}

/// Applies every migration of `migrations` missing from the history.
async fn upgrade(connection: &mut SqliteConnection, migrations: &[Migration]) -> Result<()> {
    let tables: Vec<String> = sqlx::query_scalar(
        r"SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\_%' ESCAPE '\'",
    )
    .fetch_all(&mut *connection)
    .await?;
    if tables.is_empty() {
        sqlx::query(CREATE_HISTORY)
            .execute(&mut *connection)
            .await?;
    } else if !tables.iter().any(|table| table == HISTORY_TABLE) {
        return Err(StoreError::UnversionedDatabase);
    }

    let applied = applied_migrations(connection).await?;
    if let Some(unknown) = applied.iter().find(|id| {
        MIGRATIONS
            .iter()
            .all(|migration| migration.id != id.as_str())
    }) {
        return Err(StoreError::NewerDatabase {
            migration: unknown.clone(),
        });
    }
    for migration in migrations {
        if !applied.contains(migration.id) {
            apply(connection, migration).await?;
        }
    }
    Ok(())
}

async fn applied_migrations(connection: &mut SqliteConnection) -> Result<HashSet<String>> {
    let ids: Vec<String> =
        sqlx::query_scalar(r#"SELECT "MigrationId" FROM "__EFMigrationsHistory""#)
            .fetch_all(connection)
            .await?;
    Ok(ids.into_iter().collect())
}

async fn apply(connection: &mut SqliteConnection, migration: &Migration) -> Result<()> {
    let failed = |source| StoreError::Migration {
        migration: migration.id,
        source,
    };
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
    // Another process may have applied it while this one waited for the lock.
    let already_applied: Option<i64> =
        sqlx::query_scalar(r#"SELECT 1 FROM "__EFMigrationsHistory" WHERE "MigrationId" = ?"#)
            .bind(migration.id)
            .fetch_optional(&mut *transaction)
            .await?;
    if already_applied.is_some() {
        return Ok(());
    }
    sqlx::raw_sql(migration.sql)
        .execute(&mut *transaction)
        .await
        .map_err(failed)?;
    record(&mut transaction, migration.id)
        .await
        .map_err(failed)?;
    transaction.commit().await?;
    info!(migration = migration.id, "applied database migration");
    Ok(())
}

async fn record(connection: &mut SqliteConnection, id: &str) -> sqlx::Result<()> {
    sqlx::query(
        r#"INSERT INTO "__EFMigrationsHistory" ("MigrationId", "ProductVersion") VALUES (?, ?)"#,
    )
    .bind(id)
    .bind(PRODUCT_VERSION)
    .execute(connection)
    .await?;
    Ok(())
}

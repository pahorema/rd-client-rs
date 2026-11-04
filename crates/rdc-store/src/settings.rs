//! Settings rows: one `(key, value)` row per stored setting.

use std::collections::HashSet;

use rdc_model::settings::{ApplySettingError, Settings};
use sqlx::SqlitePool;
use tracing::{debug, warn};

use crate::error::{Result, StoreError};

/// Settings operations; obtained from [`Store::settings`](crate::Store::settings).
#[derive(Debug, Clone, Copy)]
pub struct StoredSettings<'store> {
    pool: &'store SqlitePool,
}

impl<'store> StoredSettings<'store> {
    pub(crate) fn new(pool: &'store SqlitePool) -> Self {
        Self { pool }
    }

    /// Inserts every stored setting missing from the table with its default
    /// value and deletes the rows of keys that are no longer settings.
    /// Existing values are never changed.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn seed(&self) -> Result<()> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing: Vec<String> = sqlx::query_scalar(r#"SELECT "SettingId" FROM "Settings""#)
            .fetch_all(&mut *transaction)
            .await?;
        let existing: HashSet<String> = existing.into_iter().collect();
        let defaults = Settings::default();
        let mut expected = HashSet::new();
        for (key, value) in defaults.stored_rows() {
            expected.insert(key);
            if !existing.contains(key) {
                sqlx::query(r#"INSERT INTO "Settings" ("SettingId", "Value") VALUES (?, ?)"#)
                    .bind(key)
                    .bind(value)
                    .execute(&mut *transaction)
                    .await?;
            }
        }
        for key in existing
            .iter()
            .filter(|key| !expected.contains(key.as_str()))
        {
            sqlx::query(r#"DELETE FROM "Settings" WHERE "SettingId" = ?"#)
                .bind(key)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// The stored settings applied onto the defaults; see
    /// [`load_into`](Self::load_into).
    ///
    /// # Errors
    ///
    /// Fails like [`load_into`](Self::load_into).
    pub async fn load(&self) -> Result<Settings> {
        let mut settings = Settings::default();
        self.load_into(&mut settings).await?;
        Ok(settings)
    }

    /// Applies every stored row onto `settings`.
    ///
    /// A row whose value does not decode is logged and leaves that setting
    /// at its current value; rows of unknown keys are ignored.
    ///
    /// # Errors
    ///
    /// Fails with [`StoreError::NoSettings`] when the table is empty, and on
    /// database errors.
    pub async fn load_into(&self, settings: &mut Settings) -> Result<()> {
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as(r#"SELECT "SettingId", "Value" FROM "Settings""#)
                .fetch_all(self.pool)
                .await?;
        if rows.is_empty() {
            return Err(StoreError::NoSettings);
        }
        for (key, value) in &rows {
            match settings.apply(key, value.as_deref()) {
                Ok(()) => {}
                Err(ApplySettingError::InvalidValue { key, value }) => {
                    warn!("Invalid value for setting {key}: {value}");
                }
                Err(ApplySettingError::UnknownKey { key }) => {
                    debug!(%key, "ignored a stored row that is not a setting");
                }
            }
        }
        Ok(())
    }

    /// Stores the values of the given keys in one transaction; keys without
    /// a row are ignored and, for a key listed twice, the first value wins.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update(&self, values: &[(String, Option<String>)]) -> Result<()> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut written = HashSet::new();
        for (key, value) in values {
            if written.insert(key.as_str()) {
                sqlx::query(r#"UPDATE "Settings" SET "Value" = ? WHERE "SettingId" = ?"#)
                    .bind(value)
                    .bind(key)
                    .execute(&mut *transaction)
                    .await?;
            }
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Stores the value of one key; nothing happens when it has no row.
    ///
    /// # Errors
    ///
    /// Fails on database errors.
    pub async fn update_one(&self, key: &str, value: Option<&str>) -> Result<()> {
        sqlx::query(r#"UPDATE "Settings" SET "Value" = ? WHERE "SettingId" = ?"#)
            .bind(value)
            .bind(key)
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

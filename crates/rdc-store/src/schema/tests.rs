use std::path::Path;

use sqlx::{ConnectOptions, SqliteConnection, sqlite::SqliteConnectOptions};

use super::*;
use crate::Store;

async fn connect(path: &Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(false)
        .connect()
        .await
        .expect("connect")
}

/// Brings the database to the state right after migration `last`.
async fn migrate_through(connection: &mut SqliteConnection, last: &str) {
    let position = MIGRATIONS
        .iter()
        .position(|migration| migration.id == last)
        .expect("known migration");
    upgrade(connection, &MIGRATIONS[..=position])
        .await
        .expect("migrations apply");
}

async fn execute(connection: &mut SqliteConnection, sql: &'static str) {
    sqlx::raw_sql(sql)
        .execute(connection)
        .await
        .expect("sql runs");
}

async fn history(connection: &mut SqliteConnection) -> Vec<(String, String)> {
    sqlx::query_as(
        r#"SELECT "MigrationId", "ProductVersion" FROM "__EFMigrationsHistory" ORDER BY 1"#,
    )
    .fetch_all(connection)
    .await
    .expect("history")
}

async fn columns(connection: &mut SqliteConnection, table: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY name")
        .bind(table)
        .fetch_all(connection)
        .await
        .expect("table info")
}

async fn reopen(path: &Path) -> Result<()> {
    Store::open(path).await?.close().await;
    Ok(())
}

#[tokio::test]
async fn empty_database_records_every_migration() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rd-client.db");
    reopen(&path).await.expect("open");
    let expected: Vec<(String, String)> = MIGRATIONS
        .iter()
        .map(|migration| (migration.id.to_owned(), PRODUCT_VERSION.to_owned()))
        .collect();
    assert_eq!(history(&mut connect(&path).await).await, expected);
}

#[tokio::test]
async fn reopening_applies_nothing_twice() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rd-client.db");
    reopen(&path).await.expect("first open");
    reopen(&path).await.expect("second open");
    assert_eq!(
        history(&mut connect(&path).await).await.len(),
        MIGRATIONS.len()
    );
}

#[tokio::test]
async fn tables_without_history_are_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rd-client.db");
    execute(&mut connect(&path).await, "CREATE TABLE Other (Id INTEGER)").await;
    let error = reopen(&path).await.expect_err("refused");
    assert!(
        matches!(error, StoreError::UnversionedDatabase),
        "{error:?}"
    );
}

#[tokio::test]
async fn unknown_migrations_are_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rd-client.db");
    reopen(&path).await.expect("open");
    execute(
        &mut connect(&path).await,
        r#"INSERT INTO "__EFMigrationsHistory" VALUES ('29990101000000_Future', '99.0.0')"#,
    )
    .await;
    let error = reopen(&path).await.expect_err("refused");
    assert!(
        matches!(&error, StoreError::NewerDatabase { migration } if migration == "29990101000000_Future"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_failed_migration_keeps_the_previous_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rd-client.db");
    let mut connection = connect(&path).await;
    migrate_through(&mut connection, "20210110025102_Initial").await;
    execute(
        &mut connection,
        r#"ALTER TABLE "Downloads" ADD "Path" TEXT NULL"#,
    )
    .await;

    let error = reopen(&path).await.expect_err("migration fails");
    assert!(
        matches!(
            error,
            StoreError::Migration {
                migration: "20210311002748_Downloads_Add_Path",
                ..
            }
        ),
        "{error:?}"
    );
    let ids: Vec<String> = history(&mut connection)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, ["20210110025102_Initial"]);
}

#[tokio::test]
async fn rebuilt_tables_keep_their_rows() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rd-client.db");
    let mut connection = connect(&path).await;
    migrate_through(&mut connection, "20210110025102_Initial").await;
    execute(
        &mut connection,
        r#"INSERT INTO "Torrents" ("TorrentId", "Hash", "Added", "AutoDownload", "AutoUnpack",
               "AutoDelete", "RdSize", "RdSplit", "RdProgress", "RdStatus", "RdAdded", "RdName")
           VALUES ('0A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10', 'abc', '2021-01-10 02:51:02+00:00',
               1, 1, 1, 10, 2, 100, 4, '2021-01-10 02:51:02+00:00', 'Name');
           INSERT INTO "Downloads" ("DownloadId", "TorrentId", "Link", "Added")
           VALUES ('1A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10', '0A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10',
               'https://host/file', '2021-01-10 02:51:03+00:00');"#,
    )
    .await;

    reopen(&path).await.expect("upgrade");

    let torrent_columns = columns(&mut connection, "Torrents").await;
    for dropped in ["AutoDownload", "AutoUnpack", "AutoDelete"] {
        assert!(
            !torrent_columns.iter().any(|column| column == dropped),
            "{dropped} dropped"
        );
    }
    let row: (String, String, i64, i64, i64) = sqlx::query_as(
        r#"SELECT "Hash", "RdName", "IsFile", "DownloadAction", "RetryCount" FROM "Torrents""#,
    )
    .fetch_one(&mut connection)
    .await
    .expect("torrent survives");
    assert_eq!(row, ("abc".to_owned(), "Name".to_owned(), 0, 0, 0));
    let download: (String, i64) = sqlx::query_as(r#"SELECT "Link", "RetryCount" FROM "Downloads""#)
        .fetch_one(&mut connection)
        .await
        .expect("download survives");
    assert_eq!(download, ("https://host/file".to_owned(), 0));
}

async fn settings(connection: &mut SqliteConnection) -> Vec<(String, Option<String>)> {
    sqlx::query_as(r#"SELECT "SettingId", "Value" FROM "Settings" ORDER BY 1"#)
        .fetch_all(connection)
        .await
        .expect("settings")
}

#[tokio::test]
async fn setting_keys_are_grouped_and_booleans_spelled_out() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rd-client.db");
    let mut connection = connect(&path).await;
    migrate_through(&mut connection, "20210110025102_Initial").await;
    execute(
        &mut connection,
        r#"INSERT INTO "Settings" ("SettingId", "Value", "Type") VALUES
               ('DownloadPath', '/downloads', 'String'),
               ('RealDebridApiKey', 'KEY', 'String'),
               ('ProviderAutoImport', '1', 'Boolean'),
               ('ProviderAutoDelete', '0', 'Boolean'),
               ('OnlyDownloadAvailableFiles', 'True', 'Boolean'),
               ('ProviderAutoImportCategory', NULL, 'String'),
               ('TempPath', '/tmp', 'String'),
               ('Unrelated', 'x', NULL)"#,
    )
    .await;

    reopen(&path).await.expect("upgrade");

    assert_eq!(
        columns(&mut connection, "Settings").await,
        ["SettingId", "Value"]
    );
    let expected = [
        ("DownloadClient:DownloadPath", Some("/downloads")),
        ("DownloadClient:TempPath", Some("/tmp")),
        ("Provider:ApiKey", Some("KEY")),
        ("Provider:AutoDelete", Some("False")),
        ("Provider:AutoImport", Some("True")),
        ("Provider:Default:Category", None),
        ("Provider:Default:OnlyDownloadAvailableFiles", Some("True")),
        ("Unrelated", Some("x")),
    ]
    .map(|(key, value)| (key.to_owned(), value.map(str::to_owned)));
    assert_eq!(settings(&mut connection).await, expected);
}

#[tokio::test]
async fn missing_hashes_and_paths_become_empty_and_provider_fields_nullable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rd-client.db");
    let mut connection = connect(&path).await;
    migrate_through(&mut connection, "20220513175547_Settings_Migrate").await;
    execute(
        &mut connection,
        r#"INSERT INTO "Torrents" ("TorrentId", "Added", "DownloadAction", "DownloadMinSize",
               "FinishedAction", "IsFile", "RdAdded", "RdProgress", "RdSize", "RdSplit", "RdStatus")
           VALUES ('0A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10', '2022-05-14 01:05:31+00:00', 2, 5, 1, 0,
               '2022-05-14 01:05:31+00:00', 100, 10, 2, 4);
           INSERT INTO "Downloads" ("DownloadId", "TorrentId", "Added")
           VALUES ('1A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10', '0A5B6F4C-58E1-4D47-9D6F-3C6D7C8E9F10',
               '2022-05-14 01:05:32+00:00');"#,
    )
    .await;

    reopen(&path).await.expect("upgrade");

    let torrent: (String, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT "Hash", "DownloadAction", "DownloadMinSize", "HostDownloadAction",
               "DownloadClient", "FinishedActionDelay", "Type" FROM "Torrents""#,
    )
    .fetch_one(&mut connection)
    .await
    .expect("torrent survives");
    assert_eq!(torrent, (String::new(), 2, 5, 0, 0, 0, 0));
    let download: (String, Option<String>) =
        sqlx::query_as(r#"SELECT "Path", "FileName" FROM "Downloads""#)
            .fetch_one(&mut connection)
            .await
            .expect("download survives");
    assert_eq!(download, (String::new(), None));
    execute(
        &mut connection,
        r#"UPDATE "Torrents" SET "RdAdded" = NULL, "RdProgress" = NULL, "RdSize" = NULL,
               "RdSplit" = NULL, "RdStatus" = NULL"#,
    )
    .await;
    let index: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'IX_Downloads_TorrentId'",
    )
    .fetch_optional(&mut connection)
    .await
    .expect("index lookup");
    assert!(index.is_some(), "torrent index recreated");
}

#[tokio::test]
async fn stored_enum_numbers_follow_the_renumbered_values() {
    let cases = [
        ("0", "0", "1"),
        ("1", "0", "3"),
        ("2", "0", "4"),
        ("3", "2", "0"),
        ("4", "3", "0"),
        ("Bezzad", "Bezzad", "Error"),
    ];
    for (client, expected_client, log_level) in cases {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("rd-client.db");
        let mut connection = connect(&path).await;
        migrate_through(&mut connection, "20220513175547_Settings_Migrate").await;
        sqlx::query(
            r#"INSERT INTO "Settings" VALUES ('DownloadClient:Client', ?), ('General:LogLevel', ?)"#,
        )
        .bind(client)
        .bind(log_level)
        .execute(&mut connection)
        .await
        .expect("insert settings");

        reopen(&path).await.expect("upgrade");

        let expected_log_level = if log_level == "0" { "1" } else { log_level };
        let expected = [
            ("DownloadClient:Client", expected_client),
            ("General:LogLevel", expected_log_level),
        ]
        .map(|(key, value)| (key.to_owned(), Some(value.to_owned())));
        assert_eq!(settings(&mut connection).await, expected, "client {client}");
    }
}

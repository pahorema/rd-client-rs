use rdc_model::{
    DownloadClient, LogLevel, TorrentFinishedAction,
    settings::{Settings, catalogue},
};
use rdc_store::StoreError;
use sqlx::SqlitePool;

use crate::common::TestStore;

async fn rows(pool: &SqlitePool) -> Vec<(String, Option<String>)> {
    sqlx::query_as(r#"SELECT "SettingId", "Value" FROM "Settings" ORDER BY 1"#)
        .fetch_all(pool)
        .await
        .expect("settings rows")
}

async fn set_row(pool: &SqlitePool, key: &str, value: Option<&str>) {
    sqlx::query(r#"INSERT OR REPLACE INTO "Settings" ("SettingId", "Value") VALUES (?, ?)"#)
        .bind(key)
        .bind(value)
        .execute(pool)
        .await
        .expect("write setting row");
}

fn default_rows() -> Vec<(String, Option<String>)> {
    let mut rows: Vec<(String, Option<String>)> = Settings::default()
        .stored_rows()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn seeding_an_empty_table_stores_every_default() {
    let test = TestStore::new().await;
    test.store.settings().seed().await.expect("seed");
    let stored = rows(&test.raw().await).await;
    assert_eq!(stored.len(), 100);
    assert_eq!(
        stored.len(),
        catalogue()
            .iter()
            .filter(|setting| setting.is_stored())
            .count()
    );
    assert_eq!(stored, default_rows());
}

#[tokio::test]
async fn seeding_keeps_values_adds_missing_keys_and_drops_obsolete_ones() {
    let test = TestStore::new().await;
    let raw = test.raw().await;
    set_row(&raw, "General:DownloadLimit", Some("7")).await;
    set_row(&raw, "DownloadClient:TempPath", Some("/tmp")).await;

    test.store.settings().seed().await.expect("seed");
    test.store
        .settings()
        .seed()
        .await
        .expect("seeding twice changes nothing");

    let expected: Vec<(String, Option<String>)> = default_rows()
        .into_iter()
        .map(|(key, value)| {
            let value = if key == "General:DownloadLimit" {
                Some("7".to_owned())
            } else {
                value
            };
            (key, value)
        })
        .collect();
    assert_eq!(rows(&raw).await, expected);
}

#[tokio::test]
async fn loading_an_empty_table_fails() {
    let test = TestStore::new().await;
    let error = test.store.settings().load().await.expect_err("no settings");
    assert!(matches!(error, StoreError::NoSettings), "{error:?}");
    assert_eq!(error.to_string(), "No settings found, please restart");
}

#[tokio::test]
async fn loading_applies_names_numbers_and_nulls() {
    let test = TestStore::new().await;
    test.store.settings().seed().await.expect("seed");
    assert_eq!(
        test.store.settings().load().await.expect("load"),
        Settings::default()
    );

    let raw = test.raw().await;
    set_row(&raw, "General:LogLevel", Some("Debug")).await;
    set_row(&raw, "DownloadClient:Client", Some("Bezzad")).await;
    set_row(
        &raw,
        "Provider:Default:FinishedAction",
        Some("RemoveRealDebrid"),
    )
    .await;
    set_row(&raw, "Gui:Default:FinishedAction", Some("3")).await;
    set_row(&raw, "Provider:AutoImport", Some("true")).await;
    set_row(&raw, "DownloadClient:DownloadPath", None).await;
    set_row(&raw, "General:Categories", Some("tv,movies")).await;
    set_row(&raw, "Unrelated", Some("ignored")).await;

    let settings = test.store.settings().load().await.expect("load");
    assert_eq!(settings.general.log_level, LogLevel::Debug);
    assert_eq!(settings.download_client.client, DownloadClient::Internal);
    assert_eq!(
        settings.provider.default.finished_action,
        TorrentFinishedAction::RemoveProvider
    );
    assert_eq!(
        settings.gui.default.finished_action,
        TorrentFinishedAction::RemoveClient
    );
    assert!(settings.provider.auto_import);
    assert_eq!(settings.download_client.download_path, "");
    assert_eq!(settings.general.categories.as_deref(), Some("tv,movies"));
}

#[tokio::test]
async fn undecodable_rows_keep_the_current_value() {
    let test = TestStore::new().await;
    test.store.settings().seed().await.expect("seed");
    let raw = test.raw().await;
    set_row(&raw, "General:DownloadLimit", Some("many")).await;
    set_row(&raw, "Provider:AutoDelete", Some("1")).await;
    set_row(&raw, "General:UnpackLimit", Some("4")).await;

    let mut settings = Settings::default();
    settings.general.download_limit = 9;
    settings.provider.auto_delete = true;
    test.store
        .settings()
        .load_into(&mut settings)
        .await
        .expect("load");
    assert_eq!(settings.general.download_limit, 9);
    assert!(settings.provider.auto_delete);
    assert_eq!(settings.general.unpack_limit, 4);
}

#[tokio::test]
async fn updates_write_only_existing_keys() {
    let test = TestStore::new().await;
    let settings = test.store.settings();
    settings.seed().await.expect("seed");

    settings
        .update(&[
            ("General:DownloadLimit".to_owned(), Some("5".to_owned())),
            ("General:DownloadLimit".to_owned(), Some("6".to_owned())),
            ("General:Categories".to_owned(), Some(String::new())),
            ("DownloadClient:MappedPath".to_owned(), None),
            ("Not:A:Setting".to_owned(), Some("x".to_owned())),
        ])
        .await
        .expect("update");
    settings
        .update_one("Provider:ApiKey", Some("KEY"))
        .await
        .expect("update one");
    settings
        .update_one("Missing", Some("x"))
        .await
        .expect("missing keys are ignored");

    let stored = rows(&test.raw().await).await;
    let value = |key: &str| {
        stored
            .iter()
            .find(|(stored_key, _)| stored_key == key)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(value("General:DownloadLimit"), Some(Some("5".to_owned())));
    assert_eq!(value("General:Categories"), Some(Some(String::new())));
    assert_eq!(value("DownloadClient:MappedPath"), Some(None));
    assert_eq!(value("Provider:ApiKey"), Some(Some("KEY".to_owned())));
    assert_eq!(value("Not:A:Setting"), None);
    assert_eq!(value("Missing"), None);
    assert_eq!(stored.len(), 100);
}

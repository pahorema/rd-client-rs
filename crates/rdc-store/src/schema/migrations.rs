//! The ordered migrations of the database layout.
//!
//! Each migration's SQL lives in `migrations/<id>.sql`. Dropping or
//! altering a column rebuilds the table: the new table is created as
//! `rebuild_<name>`, filled from the old one, and renamed after the old
//! table is dropped.

/// One step of the database layout.
#[derive(Debug)]
pub(crate) struct Migration {
    /// Identifier recorded in the migration history.
    pub(crate) id: &'static str,
    /// Statements applying the step.
    pub(crate) sql: &'static str,
}

macro_rules! migrations {
    ($($id:literal),+ $(,)?) => {
        &[$(Migration {
            id: $id,
            sql: include_str!(concat!("../../migrations/", $id, ".sql")),
        }),+]
    };
}

/// Every migration, oldest first.
pub(crate) const MIGRATIONS: &[Migration] = migrations![
    "20210110025102_Initial",
    "20210311002748_Downloads_Add_Path",
    "20210311005539_Torrents_Removed_Auto",
    "20210313145632_Torrents_Add_FileOrMagnet",
    "20210401211556_Downloads_Add_RetryCount",
    "20210718222602_Torrents_Add_Actions",
    "20210718231758_Torrents_Add_FilesSelected",
    "20211009221133_Downloads_Add_RemoteId",
    "20211011193848_Torrent_Add_RetryCount",
    "20211024161405_Torrents_Add_Priority",
    "20211029131846_Torrents_Add_Retry",
    "20211029133429_Torrents_Error",
    "20211029142739_Torrents_Add_RetryAttempts",
    "20220206223254_Torrents_Add_DeleteOnError",
    "20220206225658_Torrents_Add_Lifetime",
    "20220513164723_Settings_Remove_Type",
    "20220513175547_Settings_Migrate",
    "20220514010531_Nullables",
    "20221018174415_Torrents_Add_HostDownloadAction",
    "20221129035837_Settings_Migrate_InternalDownloader",
    "20240105164132_Torrents_Add_DownloadClient",
    "20240213025207_Torrent_Add_Include_Exclude",
    "20240406222830_Migrate_Debug_Level",
    "20241210071621_Downloads_Add_FileName",
    "20250106112750_Torrent_Add_ClientKind",
    "20250706204358_AddFinishedActionDelay",
    "20251007021253_Settings_Migrate_InternalDownloader2",
    "20251224022148_AddDownloadTypeToTorrent",
    "20260423031719_Downloads_Add_TorrentIdPath_Unique",
    "20260528163000_MoveSourcePayloadToPayloadTable",
    "20261107000000_Torrents_Fix_Legacy_DownloadClient",
    "20261107000001_Torrents_Add_Hash_Index",
];

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
];

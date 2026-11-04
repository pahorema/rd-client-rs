CREATE TABLE "TorrentPayloads" (
    "TorrentId" TEXT NOT NULL CONSTRAINT "PK_TorrentPayloads" PRIMARY KEY,
    "Content" TEXT NOT NULL,
    CONSTRAINT "FK_TorrentPayloads_Torrents_TorrentId" FOREIGN KEY ("TorrentId") REFERENCES "Torrents" ("TorrentId")
);

INSERT INTO TorrentPayloads (TorrentId, Content)
SELECT TorrentId, FileOrMagnet
FROM Torrents
WHERE FileOrMagnet IS NOT NULL;

CREATE TABLE "rebuild_Torrents" (
    "TorrentId" TEXT NOT NULL CONSTRAINT "PK_Torrents" PRIMARY KEY,
    "Added" TEXT NOT NULL,
    "Category" TEXT NULL,
    "ClientKind" INTEGER NULL,
    "Completed" TEXT NULL,
    "DeleteOnError" INTEGER NOT NULL,
    "DownloadAction" INTEGER NOT NULL,
    "DownloadClient" INTEGER NOT NULL,
    "DownloadManualFiles" TEXT NULL,
    "DownloadMinSize" INTEGER NOT NULL,
    "DownloadRetryAttempts" INTEGER NOT NULL,
    "Error" TEXT NULL,
    "ExcludeRegex" TEXT NULL,
    "FilesSelected" TEXT NULL,
    "FinishedAction" INTEGER NOT NULL,
    "FinishedActionDelay" INTEGER NOT NULL,
    "Hash" TEXT NOT NULL,
    "HostDownloadAction" INTEGER NOT NULL,
    "IncludeRegex" TEXT NULL,
    "IsFile" INTEGER NOT NULL,
    "Lifetime" INTEGER NOT NULL,
    "Priority" INTEGER NULL,
    "RdAdded" TEXT NULL,
    "RdEnded" TEXT NULL,
    "RdFiles" TEXT NULL,
    "RdHost" TEXT NULL,
    "RdId" TEXT NULL,
    "RdName" TEXT NULL,
    "RdProgress" INTEGER NULL,
    "RdSeeders" INTEGER NULL,
    "RdSize" INTEGER NULL,
    "RdSpeed" INTEGER NULL,
    "RdSplit" INTEGER NULL,
    "RdStatus" INTEGER NULL,
    "RdStatusRaw" TEXT NULL,
    "Retry" TEXT NULL,
    "RetryCount" INTEGER NOT NULL,
    "TorrentRetryAttempts" INTEGER NOT NULL,
    "Type" INTEGER NOT NULL
);

INSERT INTO "rebuild_Torrents" ("TorrentId", "Added", "Category", "ClientKind", "Completed", "DeleteOnError", "DownloadAction", "DownloadClient", "DownloadManualFiles", "DownloadMinSize", "DownloadRetryAttempts", "Error", "ExcludeRegex", "FilesSelected", "FinishedAction", "FinishedActionDelay", "Hash", "HostDownloadAction", "IncludeRegex", "IsFile", "Lifetime", "Priority", "RdAdded", "RdEnded", "RdFiles", "RdHost", "RdId", "RdName", "RdProgress", "RdSeeders", "RdSize", "RdSpeed", "RdSplit", "RdStatus", "RdStatusRaw", "Retry", "RetryCount", "TorrentRetryAttempts", "Type")
SELECT "TorrentId", "Added", "Category", "ClientKind", "Completed", "DeleteOnError", "DownloadAction", "DownloadClient", "DownloadManualFiles", "DownloadMinSize", "DownloadRetryAttempts", "Error", "ExcludeRegex", "FilesSelected", "FinishedAction", "FinishedActionDelay", "Hash", "HostDownloadAction", "IncludeRegex", "IsFile", "Lifetime", "Priority", "RdAdded", "RdEnded", "RdFiles", "RdHost", "RdId", "RdName", "RdProgress", "RdSeeders", "RdSize", "RdSpeed", "RdSplit", "RdStatus", "RdStatusRaw", "Retry", "RetryCount", "TorrentRetryAttempts", "Type"
FROM "Torrents";

DROP TABLE "Torrents";

ALTER TABLE "rebuild_Torrents" RENAME TO "Torrents";

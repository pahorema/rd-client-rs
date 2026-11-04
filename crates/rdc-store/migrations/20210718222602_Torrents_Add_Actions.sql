ALTER TABLE "Torrents" ADD "FinishedAction" INTEGER NOT NULL DEFAULT 0;

ALTER TABLE "Torrents" ADD "DownloadAction" INTEGER NOT NULL DEFAULT 0;

ALTER TABLE "Torrents" ADD "DownloadManualFiles" TEXT NULL;

ALTER TABLE "Torrents" ADD "DownloadMinSize" INTEGER NOT NULL DEFAULT 0;

CREATE TABLE "rebuild_Torrents" (
    "TorrentId" TEXT NOT NULL CONSTRAINT "PK_Torrents" PRIMARY KEY,
    "Added" TEXT NOT NULL,
    "Category" TEXT NULL,
    "Completed" TEXT NULL,
    "DownloadAction" INTEGER NOT NULL,
    "DownloadManualFiles" TEXT NULL,
    "DownloadMinSize" INTEGER NOT NULL,
    "FileOrMagnet" TEXT NULL,
    "FinishedAction" INTEGER NOT NULL,
    "Hash" TEXT NULL,
    "IsFile" INTEGER NOT NULL,
    "RdAdded" TEXT NOT NULL,
    "RdEnded" TEXT NULL,
    "RdFiles" TEXT NULL,
    "RdHost" TEXT NULL,
    "RdId" TEXT NULL,
    "RdName" TEXT NULL,
    "RdProgress" INTEGER NOT NULL,
    "RdSeeders" INTEGER NULL,
    "RdSize" INTEGER NOT NULL,
    "RdSpeed" INTEGER NULL,
    "RdSplit" INTEGER NOT NULL,
    "RdStatus" INTEGER NOT NULL,
    "RdStatusRaw" TEXT NULL
);

INSERT INTO "rebuild_Torrents" ("TorrentId", "Added", "Category", "Completed", "DownloadAction", "DownloadManualFiles", "DownloadMinSize", "FileOrMagnet", "FinishedAction", "Hash", "IsFile", "RdAdded", "RdEnded", "RdFiles", "RdHost", "RdId", "RdName", "RdProgress", "RdSeeders", "RdSize", "RdSpeed", "RdSplit", "RdStatus", "RdStatusRaw")
SELECT "TorrentId", "Added", "Category", "Completed", "DownloadAction", "DownloadManualFiles", "DownloadMinSize", "FileOrMagnet", "FinishedAction", "Hash", "IsFile", "RdAdded", "RdEnded", "RdFiles", "RdHost", "RdId", "RdName", "RdProgress", "RdSeeders", "RdSize", "RdSpeed", "RdSplit", "RdStatus", "RdStatusRaw"
FROM "Torrents";

DROP TABLE "Torrents";

ALTER TABLE "rebuild_Torrents" RENAME TO "Torrents";

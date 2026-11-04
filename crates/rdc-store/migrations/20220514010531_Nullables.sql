CREATE TABLE "rebuild_Torrents" (
    "TorrentId" TEXT NOT NULL CONSTRAINT "PK_Torrents" PRIMARY KEY,
    "Added" TEXT NOT NULL,
    "Category" TEXT NULL,
    "Completed" TEXT NULL,
    "DeleteOnError" INTEGER NOT NULL,
    "DownloadAction" INTEGER NOT NULL,
    "DownloadManualFiles" TEXT NULL,
    "DownloadMinSize" INTEGER NOT NULL,
    "DownloadRetryAttempts" INTEGER NOT NULL,
    "Error" TEXT NULL,
    "FileOrMagnet" TEXT NULL,
    "FilesSelected" TEXT NULL,
    "FinishedAction" INTEGER NOT NULL,
    "Hash" TEXT NOT NULL,
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
    "TorrentRetryAttempts" INTEGER NOT NULL
);

INSERT INTO "rebuild_Torrents" ("TorrentId", "Added", "Category", "Completed", "DeleteOnError", "DownloadAction", "DownloadManualFiles", "DownloadMinSize", "DownloadRetryAttempts", "Error", "FileOrMagnet", "FilesSelected", "FinishedAction", "Hash", "IsFile", "Lifetime", "Priority", "RdAdded", "RdEnded", "RdFiles", "RdHost", "RdId", "RdName", "RdProgress", "RdSeeders", "RdSize", "RdSpeed", "RdSplit", "RdStatus", "RdStatusRaw", "Retry", "RetryCount", "TorrentRetryAttempts")
SELECT "TorrentId", "Added", "Category", "Completed", "DeleteOnError", "DownloadAction", "DownloadManualFiles", "DownloadMinSize", "DownloadRetryAttempts", "Error", "FileOrMagnet", "FilesSelected", "FinishedAction", IFNULL("Hash", ''), "IsFile", "Lifetime", "Priority", "RdAdded", "RdEnded", "RdFiles", "RdHost", "RdId", "RdName", "RdProgress", "RdSeeders", "RdSize", "RdSpeed", "RdSplit", "RdStatus", "RdStatusRaw", "Retry", "RetryCount", "TorrentRetryAttempts"
FROM "Torrents";

CREATE TABLE "rebuild_Downloads" (
    "DownloadId" TEXT NOT NULL CONSTRAINT "PK_Downloads" PRIMARY KEY,
    "Added" TEXT NOT NULL,
    "Completed" TEXT NULL,
    "DownloadFinished" TEXT NULL,
    "DownloadQueued" TEXT NULL,
    "DownloadStarted" TEXT NULL,
    "Error" TEXT NULL,
    "Link" TEXT NULL,
    "Path" TEXT NOT NULL,
    "RemoteId" TEXT NULL,
    "RetryCount" INTEGER NOT NULL,
    "TorrentId" TEXT NOT NULL,
    "UnpackingFinished" TEXT NULL,
    "UnpackingQueued" TEXT NULL,
    "UnpackingStarted" TEXT NULL,
    CONSTRAINT "FK_Downloads_Torrents_TorrentId" FOREIGN KEY ("TorrentId") REFERENCES "Torrents" ("TorrentId") ON DELETE RESTRICT
);

INSERT INTO "rebuild_Downloads" ("DownloadId", "Added", "Completed", "DownloadFinished", "DownloadQueued", "DownloadStarted", "Error", "Link", "Path", "RemoteId", "RetryCount", "TorrentId", "UnpackingFinished", "UnpackingQueued", "UnpackingStarted")
SELECT "DownloadId", "Added", "Completed", "DownloadFinished", "DownloadQueued", "DownloadStarted", "Error", "Link", IFNULL("Path", ''), "RemoteId", "RetryCount", "TorrentId", "UnpackingFinished", "UnpackingQueued", "UnpackingStarted"
FROM "Downloads";

DROP TABLE "Torrents";

ALTER TABLE "rebuild_Torrents" RENAME TO "Torrents";

DROP TABLE "Downloads";

ALTER TABLE "rebuild_Downloads" RENAME TO "Downloads";

CREATE INDEX "IX_Downloads_TorrentId" ON "Downloads" ("TorrentId");

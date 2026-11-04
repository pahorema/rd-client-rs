CREATE TABLE "rebuild_Torrents" (
    "TorrentId" TEXT NOT NULL CONSTRAINT "PK_Torrents" PRIMARY KEY,
    "Added" TEXT NOT NULL,
    "AutoDelete" INTEGER NOT NULL,
    "Category" TEXT NULL,
    "Completed" TEXT NULL,
    "Hash" TEXT NULL,
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

INSERT INTO "rebuild_Torrents" ("TorrentId", "Added", "AutoDelete", "Category", "Completed", "Hash", "RdAdded", "RdEnded", "RdFiles", "RdHost", "RdId", "RdName", "RdProgress", "RdSeeders", "RdSize", "RdSpeed", "RdSplit", "RdStatus", "RdStatusRaw")
SELECT "TorrentId", "Added", "AutoDelete", "Category", "Completed", "Hash", "RdAdded", "RdEnded", "RdFiles", "RdHost", "RdId", "RdName", "RdProgress", "RdSeeders", "RdSize", "RdSpeed", "RdSplit", "RdStatus", "RdStatusRaw"
FROM "Torrents";

DROP TABLE "Torrents";

ALTER TABLE "rebuild_Torrents" RENAME TO "Torrents";

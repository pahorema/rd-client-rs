DELETE FROM Downloads
WHERE rowid NOT IN (
    SELECT MIN(rowid)
    FROM Downloads
    GROUP BY TorrentId, Path
);

DROP INDEX "IX_Downloads_TorrentId";

CREATE UNIQUE INDEX "IX_Downloads_TorrentId_Path" ON "Downloads" ("TorrentId", "Path");

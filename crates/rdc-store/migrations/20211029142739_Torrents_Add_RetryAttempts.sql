ALTER TABLE "Torrents" ADD "DownloadRetryAttempts" INTEGER NOT NULL DEFAULT 0;

ALTER TABLE "Torrents" ADD "TorrentRetryAttempts" INTEGER NOT NULL DEFAULT 0;

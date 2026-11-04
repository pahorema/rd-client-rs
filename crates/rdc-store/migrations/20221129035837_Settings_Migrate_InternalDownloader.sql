UPDATE Settings SET Value = 0 WHERE SettingId = 'DownloadClient:Client' AND Value = 1;

UPDATE Settings SET Value = 1 WHERE SettingId = 'DownloadClient:Client' AND Value = 2;

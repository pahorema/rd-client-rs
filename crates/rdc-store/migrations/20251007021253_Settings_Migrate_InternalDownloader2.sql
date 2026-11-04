UPDATE Settings SET Value = 0 WHERE SettingId = 'DownloadClient:Client' AND Value = 1;

UPDATE Settings SET Value = 1 WHERE SettingId = 'DownloadClient:Client' AND Value = 2;

UPDATE Settings SET Value = 2 WHERE SettingId = 'DownloadClient:Client' AND Value = 3;

UPDATE Settings SET Value = 3 WHERE SettingId = 'DownloadClient:Client' AND Value = 4;

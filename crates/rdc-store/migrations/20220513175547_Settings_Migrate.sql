UPDATE Settings SET SettingId = 'General:LogLevel' WHERE SettingId = 'LogLevel';

UPDATE Settings SET SettingId = 'General:DownloadLimit' WHERE SettingId = 'DownloadLimit';

UPDATE Settings SET SettingId = 'General:UnpackLimit' WHERE SettingId = 'UnpackLimit';

UPDATE Settings SET SettingId = 'General:Categories' WHERE SettingId = 'Categories';

UPDATE Settings SET SettingId = 'General:RunOnTorrentCompleteFileName' WHERE SettingId = 'RunOnTorrentCompleteFileName';

UPDATE Settings SET SettingId = 'General:RunOnTorrentCompleteArguments' WHERE SettingId = 'RunOnTorrentCompleteArguments';

UPDATE Settings SET SettingId = 'DownloadClient:Client' WHERE SettingId = 'DownloadClient';

UPDATE Settings SET SettingId = 'DownloadClient:DownloadPath' WHERE SettingId = 'DownloadPath';

UPDATE Settings SET SettingId = 'DownloadClient:MappedPath' WHERE SettingId = 'MappedPath';

UPDATE Settings SET SettingId = 'DownloadClient:ChunkCount' WHERE SettingId = 'DownloadChunkCount';

UPDATE Settings SET SettingId = 'DownloadClient:MaxSpeed' WHERE SettingId = 'DownloadMaxSpeed';

UPDATE Settings SET SettingId = 'DownloadClient:Aria2cUrl' WHERE SettingId = 'Aria2cUrl';

UPDATE Settings SET SettingId = 'DownloadClient:Aria2cSecret' WHERE SettingId = 'Aria2cSecret';

UPDATE Settings SET SettingId = 'DownloadClient:TempPath' WHERE SettingId = 'TempPath';

UPDATE Settings SET SettingId = 'DownloadClient:ProxyServer' WHERE SettingId = 'ProxyServer';

UPDATE Settings SET SettingId = 'Provider:Provider' WHERE SettingId = 'Provider';

UPDATE Settings SET SettingId = 'Provider:ApiKey' WHERE SettingId = 'RealDebridApiKey';

UPDATE Settings SET SettingId = 'Provider:AutoImport' WHERE SettingId = 'ProviderAutoImport';

UPDATE Settings SET SettingId = 'Provider:AutoDelete' WHERE SettingId = 'ProviderAutoDelete';

UPDATE Settings SET SettingId = 'Provider:Timeout' WHERE SettingId = 'ProviderTimeout';

UPDATE Settings SET SettingId = 'Provider:CheckInterval' WHERE SettingId = 'ProviderCheckInterval';

UPDATE Settings SET SettingId = 'Provider:Default:Category' WHERE SettingId = 'ProviderAutoImportCategory';

UPDATE Settings SET SettingId = 'Provider:Default:OnlyDownloadAvailableFiles' WHERE SettingId = 'OnlyDownloadAvailableFiles';

UPDATE Settings SET SettingId = 'Provider:Default:MinFileSize' WHERE SettingId = 'MinFileSize';

UPDATE Settings SET SettingId = 'Provider:Default:TorrentRetryAttempts' WHERE SettingId = 'TorrentRetryAttempts';

UPDATE Settings SET SettingId = 'Provider:Default:DownloadRetryAttempts' WHERE SettingId = 'DownloadRetryAttempts';

UPDATE Settings SET SettingId = 'Provider:Default:DeleteOnError' WHERE SettingId = 'ProviderDeleteOnError';

UPDATE Settings SET SettingId = 'Provider:Default:TorrentLifetime' WHERE SettingId = 'TorrentLifetime';

UPDATE Settings SET Value = 'True' WHERE SettingId = 'Provider:AutoDelete' AND Value = '1';

UPDATE Settings SET Value = 'False' WHERE SettingId = 'Provider:AutoDelete' AND Value = '0';

UPDATE Settings SET Value = 'True' WHERE SettingId = 'Provider:AutoImport' AND Value = '1';

UPDATE Settings SET Value = 'False' WHERE SettingId = 'Provider:AutoImport' AND Value = '0';

UPDATE Settings SET Value = 'True' WHERE SettingId = 'Provider:Default:OnlyDownloadAvailableFiles' AND Value = '1';

UPDATE Settings SET Value = 'False' WHERE SettingId = 'Provider:Default:OnlyDownloadAvailableFiles' AND Value = '0';

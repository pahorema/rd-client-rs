import { describe, expect, it } from 'vitest';
import { allowedFinishedAction, guiDefaults, parseCategories } from './add-torrent.js';
import {
	DownloadClient,
	TorrentDownloadAction,
	TorrentFinishedAction,
	type Setting
} from './api/types.js';

function setting(key: string, value: Setting['value']): Setting {
	return { key, value, displayName: null, description: null, type: 'String', enumValues: null };
}

describe('guiDefaults', () => {
	it('reads the GUI defaults and the downloader', () => {
		const defaults = guiDefaults([
			setting('DownloadClient:Client', 1),
			setting('Gui:Default:Category', 'movies'),
			setting('Gui:Default:HostDownloadAction', 1),
			setting('Gui:Default:OnlyDownloadAvailableFiles', true),
			setting('Gui:Default:FinishedAction', 2),
			setting('Gui:Default:FinishedActionDelay', 5),
			setting('Gui:Default:MinFileSize', 10),
			setting('Gui:Default:IncludeRegex', 'mkv$'),
			setting('Gui:Default:ExcludeRegex', ''),
			setting('Gui:Default:TorrentRetryAttempts', 2),
			setting('Gui:Default:DownloadRetryAttempts', 4),
			setting('Gui:Default:DeleteOnError', 30),
			setting('Gui:Default:TorrentLifetime', 60),
			setting('Gui:Default:Priority', 3)
		]);

		expect(defaults).toEqual({
			category: 'movies',
			hostDownloadAction: 1,
			downloadAction: TorrentDownloadAction.DownloadAvailableFiles,
			finishedAction: 2,
			finishedActionDelay: 5,
			downloadMinSize: 10,
			includeRegex: 'mkv$',
			excludeRegex: null,
			downloadManualFiles: null,
			priority: 3,
			torrentRetryAttempts: 2,
			downloadRetryAttempts: 4,
			deleteOnError: 30,
			lifetime: 60,
			downloadClient: DownloadClient.Aria2c
		});
	});

	it('falls back when settings are missing and treats priority 0 as none', () => {
		const defaults = guiDefaults([setting('Gui:Default:Priority', 0)]);

		expect(defaults.priority).toBeNull();
		expect(defaults.downloadAction).toBe(TorrentDownloadAction.DownloadAll);
		expect(defaults.downloadRetryAttempts).toBe(3);
		expect(defaults.downloadClient).toBe(DownloadClient.Internal);
	});

	it('adjusts the finished action for the symlink downloader', () => {
		const defaults = guiDefaults([
			setting('DownloadClient:Client', DownloadClient.Symlink),
			setting('Gui:Default:FinishedAction', TorrentFinishedAction.RemoveAllTorrents)
		]);

		expect(defaults.finishedAction).toBe(TorrentFinishedAction.RemoveClient);
	});
});

describe('allowedFinishedAction', () => {
	it('keeps provider removal off for the symlink downloader', () => {
		const symlink = DownloadClient.Symlink;

		expect(
			allowedFinishedAction({
				downloadClient: symlink,
				finishedAction: TorrentFinishedAction.RemoveProvider
			})
		).toBe(TorrentFinishedAction.None);
		expect(
			allowedFinishedAction({
				downloadClient: DownloadClient.Internal,
				finishedAction: TorrentFinishedAction.RemoveProvider
			})
		).toBe(TorrentFinishedAction.RemoveProvider);
	});
});

describe('parseCategories', () => {
	it('splits, trims and removes duplicates', () => {
		expect(parseCategories(' tv, Movies,,movies , TV,anime')).toEqual(['tv', 'Movies', 'anime']);
		expect(parseCategories(null)).toEqual([]);
	});
});

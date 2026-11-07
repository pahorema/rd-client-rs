import { describe, expect, it } from 'vitest';
import { DownloadClient, TorrentHostDownloadAction, type Torrent } from './api/types.js';
import {
	applySettings,
	commonSettings,
	draftErrors,
	type TorrentSettingsDraft
} from './torrent-settings.js';

function torrent(overrides: Partial<Torrent>): Torrent {
	return {
		torrentId: 't',
		downloadClient: DownloadClient.Internal,
		hostDownloadAction: TorrentHostDownloadAction.DownloadAll,
		category: null,
		priority: null,
		downloadRetryAttempts: 3,
		torrentRetryAttempts: 1,
		deleteOnError: 0,
		lifetime: 0,
		...overrides
	} as Torrent;
}

const blank: TorrentSettingsDraft = {
	downloadClient: null,
	hostDownloadAction: null,
	category: null,
	priority: null,
	downloadRetryAttempts: null,
	torrentRetryAttempts: null,
	deleteOnError: null,
	lifetime: null
};

describe('commonSettings', () => {
	it('keeps shared values and blanks differing ones', () => {
		const draft = commonSettings([
			torrent({ category: 'tv', priority: 1, lifetime: 10 }),
			torrent({ category: 'tv', priority: 2, lifetime: 10, downloadClient: DownloadClient.Aria2c })
		]);

		expect(draft).toEqual({
			downloadClient: null,
			hostDownloadAction: TorrentHostDownloadAction.DownloadAll,
			category: 'tv',
			priority: null,
			downloadRetryAttempts: 3,
			torrentRetryAttempts: 1,
			deleteOnError: 0,
			lifetime: 10
		});
	});

	it('returns blanks for no torrents', () => {
		expect(commonSettings([])).toEqual(blank);
	});
});

describe('applySettings', () => {
	it('leaves blank values unchanged in bulk mode', () => {
		const original = torrent({ category: 'tv', priority: 4, lifetime: 5 });

		expect(applySettings(original, { ...blank, lifetime: 60 }, 'bulk')).toEqual({
			...original,
			lifetime: 60
		});
	});

	it('clears category and priority in single mode', () => {
		const original = torrent({ category: 'tv', priority: 4 });
		const next = applySettings(
			original,
			{ ...commonSettings([original]), category: ' ', priority: null },
			'single'
		);

		expect(next.category).toBeNull();
		expect(next.priority).toBeNull();
	});

	it('applies every entered value without mutating the input', () => {
		const original = torrent({});
		const next = applySettings(
			original,
			{
				downloadClient: DownloadClient.Symlink,
				hostDownloadAction: TorrentHostDownloadAction.DownloadNone,
				category: ' movies ',
				priority: 2,
				downloadRetryAttempts: 5,
				torrentRetryAttempts: 2,
				deleteOnError: 30,
				lifetime: 120
			},
			'bulk'
		);

		expect(next).toMatchObject({
			downloadClient: DownloadClient.Symlink,
			hostDownloadAction: TorrentHostDownloadAction.DownloadNone,
			category: 'movies',
			priority: 2,
			downloadRetryAttempts: 5,
			torrentRetryAttempts: 2,
			deleteOnError: 30,
			lifetime: 120
		});
		expect(original.category).toBeNull();
	});
});

describe('draftErrors', () => {
	it('allows blank values and rejects decimals and negatives', () => {
		const draft = commonSettings([]);

		expect(draftErrors(draft)).toEqual({});
		expect(Object.keys(draftErrors({ ...draft, lifetime: 1.5, priority: -2 }))).toEqual([
			'priority',
			'lifetime'
		]);
	});
});

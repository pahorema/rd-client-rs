import { describe, expect, it } from 'vitest';
import { TorrentStatus, type Torrent } from './api/types.js';
import {
	browserStorage,
	defaultTorrentSort,
	filterTorrents,
	loadTorrentSort,
	nextSort,
	saveTorrentSort,
	sortTorrents,
	torrentState
} from './torrent-list.js';

function torrent(overrides: Partial<Torrent>): Torrent {
	return {
		torrentId: 't',
		hash: 'abc',
		rdName: 'Name',
		category: null,
		statusText: 'Queued for downloading',
		error: null,
		completed: null,
		rdStatus: TorrentStatus.Downloading,
		added: '2025-01-01T00:00:00+00:00',
		...overrides
	} as Torrent;
}

const active = torrent({ torrentId: 'a', rdName: 'Alpha.Show', category: 'tv' });
const done = torrent({
	torrentId: 'b',
	rdName: 'Beta.Movie',
	completed: '2025-01-02T00:00:00+00:00',
	statusText: 'Finished',
	added: '2025-01-03T00:00:00.1234567+00:00'
});
const failed = torrent({ torrentId: 'c', rdName: 'Gamma', error: 'Boom', statusText: 'Boom' });
const providerError = torrent({ torrentId: 'd', rdStatus: TorrentStatus.Error });

describe('torrentState', () => {
	it('classifies torrents', () => {
		expect(torrentState(active)).toBe('active');
		expect(torrentState(done)).toBe('completed');
		expect(torrentState(failed)).toBe('error');
		expect(torrentState(providerError)).toBe('error');
		expect(torrentState({ ...done, error: 'Torrent deleted' })).toBe('error');
	});
});

describe('filterTorrents', () => {
	const all = [active, done, failed];

	it('filters by state', () => {
		expect(filterTorrents(all, '', 'all')).toEqual(all);
		expect(filterTorrents(all, '', 'completed')).toEqual([done]);
		expect(filterTorrents(all, '', 'error')).toEqual([failed]);
	});

	it('searches name, category, hash and status case-insensitively', () => {
		expect(filterTorrents(all, 'beta', 'all')).toEqual([done]);
		expect(filterTorrents(all, ' TV ', 'all')).toEqual([active]);
		expect(filterTorrents(all, 'boom', 'all')).toEqual([failed]);
		expect(filterTorrents(all, 'ABC', 'active')).toEqual([active]);
	});
});

describe('sortTorrents', () => {
	it('sorts by added time and by status text', () => {
		const byAdded = sortTorrents([active, done, failed], defaultTorrentSort);
		expect(byAdded[0]).toBe(done);

		const byStatus = sortTorrents([active, done, failed], { field: 'status', direction: 'asc' });
		expect(byStatus.map((t) => t.statusText)).toEqual([
			'Boom',
			'Finished',
			'Queued for downloading'
		]);
	});
});

describe('nextSort', () => {
	it('toggles the direction of the current column', () => {
		expect(nextSort({ field: 'rdName', direction: 'desc' }, 'rdName')).toEqual({
			field: 'rdName',
			direction: 'asc'
		});
	});

	it('starts a new column descending', () => {
		expect(nextSort({ field: 'rdName', direction: 'asc' }, 'rdSize')).toEqual({
			field: 'rdSize',
			direction: 'desc'
		});
	});
});

describe('sort persistence', () => {
	it('round-trips through storage', () => {
		const values = new Map<string, string>();
		const storage = {
			getItem: (key: string) => values.get(key) ?? null,
			setItem: (key: string, value: string) => void values.set(key, value)
		};

		saveTorrentSort(storage, { field: 'rdSize', direction: 'asc' });

		expect(loadTorrentSort(storage)).toEqual({ field: 'rdSize', direction: 'asc' });
	});

	it('ignores missing or invalid values', () => {
		expect(loadTorrentSort(undefined)).toEqual(defaultTorrentSort);
		expect(loadTorrentSort({ getItem: () => '{"field":"nope","direction":"asc"}' })).toEqual(
			defaultTorrentSort
		);
		expect(loadTorrentSort({ getItem: () => 'not json' })).toEqual(defaultTorrentSort);
	});

	it('treats storage whose getter throws as unavailable', () => {
		const blocked = {
			get localStorage(): Storage {
				throw new DOMException('The operation is insecure.', 'SecurityError');
			}
		};

		expect(browserStorage(blocked)).toBeUndefined();
		expect(loadTorrentSort(browserStorage(blocked))).toEqual(defaultTorrentSort);
	});
});

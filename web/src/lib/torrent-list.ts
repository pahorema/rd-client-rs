import { TorrentStatus, type Torrent } from './api/types.js';
import { parseTimestamp } from './format.js';
import { getSortFieldValue, sortItems, type SortDirection } from './sort.js';

export type TorrentState = 'active' | 'completed' | 'error';
export type TorrentFilter = 'all' | TorrentState;

/** Columns the torrent table can be sorted by. */
export type TorrentSortField =
	| 'rdName'
	| 'category'
	| 'priority'
	| 'rdSeeders'
	| 'filesCount'
	| 'downloadsCount'
	| 'rdSize'
	| 'added'
	| 'status';

export interface TorrentSort {
	field: TorrentSortField;
	direction: SortDirection;
}

export const defaultTorrentSort: TorrentSort = { field: 'added', direction: 'desc' };

const SORT_FIELDS: readonly TorrentSortField[] = [
	'rdName',
	'category',
	'priority',
	'rdSeeders',
	'filesCount',
	'downloadsCount',
	'rdSize',
	'added',
	'status'
];

/** Whether a torrent is in error, completed, or still being processed. */
export function torrentState(torrent: Torrent): TorrentState {
	if (torrent.error || torrent.rdStatus === TorrentStatus.Error) {
		return 'error';
	}

	return torrent.completed ? 'completed' : 'active';
}

/** Torrents matching `filter` whose name, category, hash or status contains `query`. */
export function filterTorrents(
	torrents: readonly Torrent[],
	query: string,
	filter: TorrentFilter
): Torrent[] {
	const needle = query.trim().toLowerCase();

	return torrents.filter((torrent) => {
		if (filter !== 'all' && torrentState(torrent) !== filter) {
			return false;
		}

		if (!needle) {
			return true;
		}

		return [torrent.rdName, torrent.category, torrent.hash, torrent.statusText].some((value) =>
			value?.toLowerCase().includes(needle)
		);
	});
}

export function sortTorrents(torrents: readonly Torrent[], sort: TorrentSort): Torrent[] {
	return sortItems(torrents, sort.field, sort.direction, (torrent, field) => {
		switch (field) {
			case 'added':
				return parseTimestamp(torrent.added);
			case 'status':
				return torrent.statusText;
			default:
				return getSortFieldValue(torrent, field);
		}
	});
}

/** Clicking the current column flips the direction; a new column starts descending. */
export function nextSort(current: TorrentSort, field: TorrentSortField): TorrentSort {
	if (current.field === field) {
		return { field, direction: current.direction === 'asc' ? 'desc' : 'asc' };
	}

	return { field, direction: 'desc' };
}

const SORT_STORAGE_KEY = 'rd-client.torrents.sort';

/** Reads the persisted sort order, falling back to newest first. */
export function loadTorrentSort(storage: Pick<Storage, 'getItem'> | undefined): TorrentSort {
	try {
		const parsed = JSON.parse(storage?.getItem(SORT_STORAGE_KEY) ?? 'null') as TorrentSort | null;

		if (
			parsed &&
			SORT_FIELDS.includes(parsed.field) &&
			(parsed.direction === 'asc' || parsed.direction === 'desc')
		) {
			return parsed;
		}
	} catch {
		// Unreadable storage keeps the default order.
	}

	return defaultTorrentSort;
}

export function saveTorrentSort(storage: Pick<Storage, 'setItem'> | undefined, sort: TorrentSort) {
	try {
		storage?.setItem(SORT_STORAGE_KEY, JSON.stringify(sort));
	} catch {
		// Storage may be disabled; sorting still works for this session.
	}
}

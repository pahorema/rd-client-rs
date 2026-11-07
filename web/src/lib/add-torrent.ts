import {
	DownloadClient,
	TorrentDownloadAction,
	TorrentFinishedAction,
	TorrentHostDownloadAction,
	type NewTorrent,
	type Setting
} from './api/types.js';
import { wholeNumberError } from './numbers.js';

/** Numeric settings of a new torrent; only the priority may be left empty. */
export const NUMBER_FIELDS = [
	'priority',
	'downloadMinSize',
	'finishedActionDelay',
	'downloadRetryAttempts',
	'torrentRetryAttempts',
	'deleteOnError',
	'lifetime'
] as const satisfies readonly (keyof NewTorrent)[];

export type NumberField = (typeof NUMBER_FIELDS)[number];

/**
 * Errors of numeric fields the server would reject: empty required fields, decimals and
 * negative numbers. Keys follow `NUMBER_FIELDS` order, so the first key is the first bad field.
 */
export function newTorrentErrors(torrent: NewTorrent): Partial<Record<NumberField, string>> {
	const errors: Partial<Record<NumberField, string>> = {};

	for (const field of NUMBER_FIELDS) {
		const error = wholeNumberError(torrent[field], { optional: field === 'priority' });

		if (error) {
			errors[field] = error;
		}
	}

	return errors;
}

/** Per-torrent settings for a new torrent, taken from the `Gui:Default:*` settings. */
export function guiDefaults(settings: readonly Setting[]): NewTorrent {
	const value = (key: string) => settings.find((s) => s.key === key)?.value;
	const number = (key: string, fallback: number) => {
		const raw = Number(value(key));
		return Number.isFinite(raw) && value(key) != null ? raw : fallback;
	};
	const text = (key: string) => {
		const raw = value(key);
		return typeof raw === 'string' && raw.trim() ? raw : null;
	};
	const priority = number('Gui:Default:Priority', 0);

	const defaults: NewTorrent = {
		category: text('Gui:Default:Category'),
		hostDownloadAction: number(
			'Gui:Default:HostDownloadAction',
			TorrentHostDownloadAction.DownloadAll
		) as TorrentHostDownloadAction,
		downloadAction:
			value('Gui:Default:OnlyDownloadAvailableFiles') === true
				? TorrentDownloadAction.DownloadAvailableFiles
				: TorrentDownloadAction.DownloadAll,
		finishedAction: number(
			'Gui:Default:FinishedAction',
			TorrentFinishedAction.None
		) as TorrentFinishedAction,
		finishedActionDelay: number('Gui:Default:FinishedActionDelay', 0),
		downloadMinSize: number('Gui:Default:MinFileSize', 0),
		includeRegex: text('Gui:Default:IncludeRegex'),
		excludeRegex: text('Gui:Default:ExcludeRegex'),
		downloadManualFiles: null,
		priority: priority > 0 ? priority : null,
		torrentRetryAttempts: number('Gui:Default:TorrentRetryAttempts', 1),
		downloadRetryAttempts: number('Gui:Default:DownloadRetryAttempts', 3),
		deleteOnError: number('Gui:Default:DeleteOnError', 0),
		lifetime: number('Gui:Default:TorrentLifetime', 0),
		downloadClient: number('DownloadClient:Client', DownloadClient.Internal) as DownloadClient
	};

	return { ...defaults, finishedAction: allowedFinishedAction(defaults) };
}

/**
 * The symlink downloader keeps the files on the provider, so actions removing
 * the torrent there become their client-only equivalent.
 */
export function allowedFinishedAction(
	torrent: Pick<NewTorrent, 'downloadClient' | 'finishedAction'>
): TorrentFinishedAction {
	if (torrent.downloadClient !== DownloadClient.Symlink) {
		return torrent.finishedAction;
	}

	switch (torrent.finishedAction) {
		case TorrentFinishedAction.RemoveAllTorrents:
			return TorrentFinishedAction.RemoveClient;
		case TorrentFinishedAction.RemoveProvider:
			return TorrentFinishedAction.None;
		default:
			return torrent.finishedAction;
	}
}

/** Comma-separated categories from `General:Categories`, without blanks or case-insensitive duplicates. */
export function parseCategories(raw: unknown): string[] {
	const seen = new Set<string>();

	return (typeof raw === 'string' ? raw : '')
		.split(',')
		.map((c) => c.trim())
		.filter((c) => {
			const key = c.toLowerCase();
			if (!c || seen.has(key)) return false;
			seen.add(key);
			return true;
		});
}

/**
 * Magnet link from a `#/add?magnet=…` fragment. The router may decode the
 * fragment, which turns the magnet's own `&` into separators, so the value runs
 * to the end of the fragment and is decoded only while still percent-encoded.
 */
export function magnetFromHash(hash: string): string | null {
	const match = /[?&]magnet=(.*)$/.exec(hash);

	if (!match) {
		return null;
	}

	let value = match[1];

	try {
		while (/^magnet%3a/i.test(value)) {
			value = decodeURIComponent(value);
		}
	} catch {
		return value;
	}

	return value || null;
}

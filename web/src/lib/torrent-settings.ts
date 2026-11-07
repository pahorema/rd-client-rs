import type { Torrent, TorrentSettings } from './api/types.js';
import { wholeNumberError } from './numbers.js';

/** Editable torrent settings; `null` means "keep the current value" when editing several torrents. */
export type TorrentSettingsDraft = { [K in keyof TorrentSettings]: TorrentSettings[K] | null };

const KEYS = [
	'downloadClient',
	'hostDownloadAction',
	'category',
	'priority',
	'downloadRetryAttempts',
	'torrentRetryAttempts',
	'deleteOnError',
	'lifetime'
] as const satisfies readonly (keyof TorrentSettings)[];

/** The settings shared by every torrent; settings that differ are `null`. */
export function commonSettings(torrents: readonly Torrent[]): TorrentSettingsDraft {
	const draft = {} as Record<keyof TorrentSettings, unknown>;

	for (const key of KEYS) {
		const first = torrents[0]?.[key] ?? null;
		draft[key] = torrents.every((torrent) => (torrent[key] ?? null) === first) ? first : null;
	}

	return draft as TorrentSettingsDraft;
}

const NUMBER_KEYS = [
	'priority',
	'downloadRetryAttempts',
	'torrentRetryAttempts',
	'deleteOnError',
	'lifetime'
] as const satisfies readonly (keyof TorrentSettings)[];

export type SettingsNumberField = (typeof NUMBER_KEYS)[number];

/** Numeric values the server would reject; blank values are valid (see `applySettings`). */
export function draftErrors(
	draft: TorrentSettingsDraft
): Partial<Record<SettingsNumberField, string>> {
	const errors: Partial<Record<SettingsNumberField, string>> = {};

	for (const key of NUMBER_KEYS) {
		const error = wholeNumberError(draft[key], { optional: true });

		if (error) {
			errors[key] = error;
		}
	}

	return errors;
}

/**
 * Applies `draft` to `torrent`. For a single torrent every value is taken as
 * entered (a blank category or priority clears it); for several torrents blank
 * values leave the torrent unchanged.
 */
export function applySettings(
	torrent: Torrent,
	draft: TorrentSettingsDraft,
	mode: 'single' | 'bulk'
): Torrent {
	const next: Torrent = { ...torrent };
	const category = draft.category?.trim() ? draft.category.trim() : null;

	if (mode === 'single') {
		next.category = category;
		next.priority = draft.priority ?? null;
	} else {
		if (category != null) next.category = category;
		if (draft.priority != null) next.priority = draft.priority;
	}

	if (draft.downloadClient != null) next.downloadClient = draft.downloadClient;
	if (draft.hostDownloadAction != null) next.hostDownloadAction = draft.hostDownloadAction;
	if (draft.downloadRetryAttempts != null) next.downloadRetryAttempts = draft.downloadRetryAttempts;
	if (draft.torrentRetryAttempts != null) next.torrentRetryAttempts = draft.torrentRetryAttempts;
	if (draft.deleteOnError != null) next.deleteOnError = draft.deleteOnError;
	if (draft.lifetime != null) next.lifetime = draft.lifetime;

	return next;
}

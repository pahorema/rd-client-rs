import { describe, expect, it } from 'vitest';
import {
	DownloadClient,
	TorrentDownloadAction,
	TorrentFinishedAction,
	TorrentHostDownloadAction,
	TorrentStatus
} from './api/types.js';
import {
	downloadActionOptions,
	downloadClientOptions,
	finishedActionOptions,
	hostDownloadActionOptions,
	optionLabel,
	providerStatusLabel
} from './labels.js';

describe('labels', () => {
	it('labels every enum value', () => {
		const cases = [
			[downloadClientOptions, DownloadClient],
			[hostDownloadActionOptions, TorrentHostDownloadAction],
			[downloadActionOptions, TorrentDownloadAction],
			[finishedActionOptions, TorrentFinishedAction]
		] as const;

		for (const [options, values] of cases) {
			expect(options.map((option) => option.value)).toEqual(Object.values(values));
		}

		for (const status of Object.values(TorrentStatus)) {
			expect(providerStatusLabel(status)).not.toBe('Unknown');
		}
	});

	it('calls the internal downloader by its product name', () => {
		expect(optionLabel(downloadClientOptions, DownloadClient.Internal)).toBe('Internal downloader');
	});

	it('falls back for unknown values', () => {
		expect(optionLabel(downloadClientOptions, null)).toBe('');
		expect(providerStatusLabel(null)).toBe('Unknown');
	});
});

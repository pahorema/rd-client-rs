import { describe, expect, it } from 'vitest';
import type { Download } from './api/types.js';
import { downloadStatusText, torrentProgress } from './status.js';

const T = '2025-01-31T10:00:00+00:00';

function download(overrides: Partial<Download> = {}): Download {
	return {
		downloadId: 'd1',
		torrentId: 't1',
		path: 'https://provider/d/1',
		link: null,
		added: T,
		downloadQueued: null,
		downloadStarted: null,
		downloadFinished: null,
		unpackingQueued: null,
		unpackingStarted: null,
		unpackingFinished: null,
		completed: null,
		retryCount: 0,
		error: null,
		size: null,
		bytesTotal: 0,
		bytesDone: 0,
		speed: 0,
		...overrides
	};
}

function torrent(overrides: Partial<Parameters<typeof torrentProgress>[0]> = {}) {
	return {
		error: null,
		completed: null,
		rdProgress: null,
		downloads: [],
		...overrides
	};
}

describe('downloadStatusText', () => {
	it('handles a missing download', () => {
		expect(downloadStatusText(null)).toBe('Pending');
	});

	it('walks the download lifecycle', () => {
		expect(downloadStatusText(download())).toBe('Pending');
		expect(downloadStatusText(download({ downloadQueued: T }))).toBe('Download queued');
		expect(
			downloadStatusText(
				download({ downloadStarted: T, bytesDone: 50, bytesTotal: 200, speed: 2048 })
			)
		).toBe('Downloading 25.00% (2 KB/s)');
		expect(downloadStatusText(download({ downloadStarted: T }))).toBe('Downloading 0.00% (0 B/s)');
		expect(downloadStatusText(download({ downloadFinished: T }))).toBe('Download finished');
		expect(downloadStatusText(download({ unpackingQueued: T }))).toBe('Unpacking queued');
		expect(
			downloadStatusText(download({ unpackingStarted: T, bytesDone: 10, bytesTotal: 100 }))
		).toBe('Unpacking 10.00%');
		expect(downloadStatusText(download({ unpackingFinished: T }))).toBe('Unpacking finished');
		expect(downloadStatusText(download({ completed: T }))).toBe('Finished');
	});

	it('prefers the error text', () => {
		expect(downloadStatusText(download({ error: 'Link expired', completed: T }))).toBe(
			'Link expired'
		);
	});
});

describe('torrentProgress', () => {
	it('uses the provider progress before any download exists', () => {
		expect(torrentProgress(torrent({ rdProgress: 37 }))).toBe(37);
		expect(torrentProgress(torrent())).toBe(0);
	});

	it('is complete once the torrent is completed without error', () => {
		expect(torrentProgress(torrent({ completed: T }))).toBe(100);
	});

	it('averages the host progress of each download', () => {
		const downloads = [
			download({ completed: T }),
			download({ downloadStarted: T, bytesDone: 50, bytesTotal: 100 }),
			download()
		];

		expect(torrentProgress(torrent({ downloads }))).toBe(50);
	});

	it('keeps a transferred download at 100% until it is completed', () => {
		const stages = [
			download({ downloadStarted: T, downloadFinished: T }),
			download({ downloadStarted: T, downloadFinished: T, unpackingQueued: T }),
			// Unpacking reports its own percentage in the byte counters.
			download({
				downloadStarted: T,
				downloadFinished: T,
				unpackingStarted: T,
				bytesDone: 5,
				bytesTotal: 100
			})
		];

		for (const stage of stages) {
			expect(torrentProgress(torrent({ downloads: [stage] }))).toBe(100);
		}
	});
});

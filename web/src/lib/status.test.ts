import { describe, expect, it } from 'vitest';
import { TorrentStatus, type Download } from './api/types.js';
import { downloadStatusText, torrentProgress, torrentStatusText } from './status.js';

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
		bytesTotal: 0,
		bytesDone: 0,
		speed: 0,
		...overrides
	};
}

function torrent(overrides: Partial<Parameters<typeof torrentStatusText>[0]> = {}) {
	return {
		error: null,
		completed: null,
		rdStatus: null,
		rdStatusRaw: null,
		rdProgress: null,
		rdSpeed: null,
		rdSeeders: null,
		downloads: [],
		...overrides
	};
}

describe('torrentStatusText', () => {
	it('prefers the error text', () => {
		expect(torrentStatusText(torrent({ error: 'Boom', completed: T }))).toBe('Boom');
	});

	it('reports finished when every download is completed', () => {
		const downloads = [download({ completed: T }), download({ completed: T })];

		expect(torrentStatusText(torrent({ downloads }))).toBe('Finished');
	});

	it('sums active downloads with their speed', () => {
		const downloads = [
			download({ downloadStarted: T, bytesDone: 256, bytesTotal: 1024, speed: 1024 * 1024 }),
			download({ downloadStarted: T, bytesDone: 256, bytesTotal: 1024, speed: 1024 * 1024 }),
			download({ downloadStarted: T, downloadFinished: T, completed: T })
		];

		expect(torrentStatusText(torrent({ downloads }))).toBe(
			'Downloading file 3/3 (25.00% - 2 MB/s)'
		);
	});

	it('reports extraction progress', () => {
		const downloads = [
			download({
				downloadStarted: T,
				downloadFinished: T,
				unpackingQueued: T,
				unpackingStarted: T,
				bytesDone: 40,
				bytesTotal: 100
			}),
			download({ downloadFinished: T, unpackingFinished: T, completed: T })
		];

		expect(torrentStatusText(torrent({ downloads }))).toBe('Extracting file 2/2 (40.00%)');
	});

	it('reports queued stages in order', () => {
		expect(
			torrentStatusText(
				torrent({
					downloads: [download({ downloadStarted: T, downloadFinished: T, unpackingQueued: T })]
				})
			)
		).toBe('Queued for unpacking');
		expect(torrentStatusText(torrent({ downloads: [download()] }))).toBe('Queued for downloading');
	});

	it('reports host-side results when nothing is active', () => {
		expect(
			torrentStatusText(
				torrent({
					downloads: [download({ downloadStarted: T, downloadFinished: T, unpackingFinished: T })]
				})
			)
		).toBe('Files unpacked');
		expect(
			torrentStatusText(
				torrent({ downloads: [download({ downloadStarted: T, downloadFinished: T })] })
			)
		).toBe('Files downloaded to host');
	});

	it('falls back to the completion time', () => {
		expect(torrentStatusText(torrent({ completed: T }))).toBe('Finished');
	});

	it('describes every provider status', () => {
		const cases: [TorrentStatus | null, string][] = [
			[TorrentStatus.Queued, 'Not Yet Added to Provider'],
			[TorrentStatus.Processing, 'Torrent processing'],
			[TorrentStatus.WaitingForFileSelection, 'Torrent waiting for file selection'],
			[TorrentStatus.Finished, 'Torrent finished, waiting for download links'],
			[TorrentStatus.Uploading, 'Torrent uploading'],
			[null, 'Unknown status']
		];

		for (const [rdStatus, text] of cases) {
			expect(torrentStatusText(torrent({ rdStatus }))).toBe(text);
		}

		expect(
			torrentStatusText(torrent({ rdStatus: TorrentStatus.Error, rdStatusRaw: 'magnet_error' }))
		).toBe('Torrent error: magnet_error');
	});

	it('reports provider download progress and stalls', () => {
		expect(
			torrentStatusText(
				torrent({
					rdStatus: TorrentStatus.Downloading,
					rdProgress: 42,
					rdSpeed: 1536,
					rdSeeders: 3
				})
			)
		).toBe('Torrent downloading (42% - 1.5 KB/s)');
		expect(torrentStatusText(torrent({ rdStatus: TorrentStatus.Downloading, rdSeeders: 0 }))).toBe(
			'Torrent stalled'
		);
		expect(
			torrentStatusText(torrent({ rdStatus: TorrentStatus.Downloading, rdSeeders: null }))
		).toBe('Torrent downloading (% - 0 B/s)');
	});
});

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

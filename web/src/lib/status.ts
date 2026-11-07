import type { Download, Torrent } from './api/types.js';
import { formatSpeed } from './format.js';

type TorrentProgressInput = Pick<Torrent, 'error' | 'completed' | 'rdProgress'> & {
	downloads?: readonly Download[] | null;
};

function percentage(done: number, total: number): string {
	return ((done / total || 0) * 100).toFixed(2);
}

/** Human-readable status of a single download. */
export function downloadStatusText(download: Download | null | undefined): string {
	if (!download) {
		return 'Pending';
	}

	if (download.error) {
		return download.error;
	}

	if (download.completed != null) {
		return 'Finished';
	}

	if (download.unpackingFinished) {
		return 'Unpacking finished';
	}

	if (download.unpackingStarted) {
		return `Unpacking ${percentage(download.bytesDone, download.bytesTotal)}%`;
	}

	if (download.unpackingQueued) {
		return 'Unpacking queued';
	}

	if (download.downloadFinished) {
		return 'Download finished';
	}

	if (download.downloadStarted) {
		return `Downloading ${percentage(download.bytesDone, download.bytesTotal)}% (${formatSpeed(download.speed)})`;
	}

	if (download.downloadQueued) {
		return 'Download queued';
	}

	return 'Pending';
}

/**
 * Overall progress of a torrent from 0 to 100: the provider progress until
 * files reach the host, then the host transfer progress. A transferred file
 * counts as done while it waits for or goes through unpacking, whose live
 * counters measure the archive rather than the transfer.
 */
export function torrentProgress(torrent: TorrentProgressInput): number {
	if (torrent.completed && !torrent.error) {
		return 100;
	}

	const downloads = torrent.downloads ?? [];

	if (downloads.length === 0) {
		return Math.min(Math.max(torrent.rdProgress ?? 0, 0), 100);
	}

	let total = 0;

	for (const download of downloads) {
		if (download.downloadFinished != null || download.completed != null) {
			total += 1;
		} else if (download.downloadStarted) {
			total += download.bytesTotal > 0 ? Math.min(download.bytesDone / download.bytesTotal, 1) : 0;
		}
	}

	return (total / downloads.length) * 100;
}

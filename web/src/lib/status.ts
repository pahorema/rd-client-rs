import { TorrentStatus, type Download, type Torrent } from './api/types.js';
import { formatSpeed } from './format.js';

type TorrentStatusInput = Pick<
	Torrent,
	'error' | 'completed' | 'rdStatus' | 'rdStatusRaw' | 'rdProgress' | 'rdSpeed' | 'rdSeeders'
> & { downloads?: readonly Download[] | null };

function percentage(done: number, total: number): string {
	return ((done / total || 0) * 100).toFixed(2);
}

function downloadsStatusText(downloads: readonly Download[]): string | null {
	let allFinished = true;
	let downloadingCount = 0;
	let downloadedCount = 0;
	let downloadingDone = 0;
	let downloadingTotal = 0;
	let downloadingSpeed = 0;
	let unpackingCount = 0;
	let unpackedCount = 0;
	let unpackingDone = 0;
	let unpackingTotal = 0;
	let queuedForUnpackingCount = 0;
	let queuedForDownloadingCount = 0;

	for (const download of downloads) {
		if (download.completed == null) {
			allFinished = false;
		}

		if (download.downloadFinished != null) {
			downloadedCount += 1;
		}

		if (download.downloadStarted && !download.downloadFinished && download.bytesDone > 0) {
			downloadingCount += 1;
			downloadingDone += download.bytesDone;
			downloadingTotal += download.bytesTotal;
			downloadingSpeed += download.speed;
		}

		if (download.unpackingFinished != null) {
			unpackedCount += 1;
		}

		if (download.unpackingStarted && !download.unpackingFinished && download.bytesDone > 0) {
			unpackingCount += 1;
			unpackingDone += download.bytesDone;
			unpackingTotal += download.bytesTotal;
		}

		if (download.unpackingQueued && !download.unpackingStarted) {
			queuedForUnpackingCount += 1;
		}

		if (!download.downloadStarted && !download.downloadFinished) {
			queuedForDownloadingCount += 1;
		}
	}

	const count = downloads.length;

	if (allFinished) {
		return 'Finished';
	}

	if (downloadingCount > 0) {
		const progress = percentage(downloadingDone, downloadingTotal);

		return `Downloading file ${downloadingCount + downloadedCount}/${count} (${progress}% - ${formatSpeed(downloadingSpeed)})`;
	}

	if (unpackingCount > 0) {
		const progress = percentage(unpackingDone, unpackingTotal);

		return `Extracting file ${unpackingCount + unpackedCount}/${count} (${progress}%)`;
	}

	if (queuedForUnpackingCount > 0) {
		return 'Queued for unpacking';
	}

	if (queuedForDownloadingCount > 0) {
		return 'Queued for downloading';
	}

	if (unpackedCount > 0) {
		return 'Files unpacked';
	}

	if (downloadedCount > 0) {
		return 'Files downloaded to host';
	}

	return null;
}

/** Human-readable status of a torrent, from its error, downloads and provider state. */
export function torrentStatusText(torrent: TorrentStatusInput): string {
	if (torrent.error) {
		return torrent.error;
	}

	const downloads = torrent.downloads ?? [];

	if (downloads.length > 0) {
		const text = downloadsStatusText(downloads);

		if (text != null) {
			return text;
		}
	}

	if (torrent.completed) {
		return 'Finished';
	}

	switch (torrent.rdStatus) {
		case TorrentStatus.Queued:
			return 'Not Yet Added to Provider';
		case TorrentStatus.Downloading:
			if (torrent.rdSeeders != null && torrent.rdSeeders < 1) {
				return 'Torrent stalled';
			}

			return `Torrent downloading (${torrent.rdProgress ?? ''}% - ${formatSpeed(torrent.rdSpeed)})`;
		case TorrentStatus.Processing:
			return 'Torrent processing';
		case TorrentStatus.WaitingForFileSelection:
			return 'Torrent waiting for file selection';
		case TorrentStatus.Error:
			return `Torrent error: ${torrent.rdStatusRaw ?? ''}`;
		case TorrentStatus.Finished:
			return 'Torrent finished, waiting for download links';
		case TorrentStatus.Uploading:
			return 'Torrent uploading';
		default:
			return 'Unknown status';
	}
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
export function torrentProgress(torrent: TorrentStatusInput): number {
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

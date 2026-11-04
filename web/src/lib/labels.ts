import {
	DownloadClient,
	TorrentDownloadAction,
	TorrentFinishedAction,
	TorrentHostDownloadAction,
	TorrentStatus
} from './api/types.js';
import type { Option } from './components/option-select.svelte';

export const downloadClientOptions: readonly Option<DownloadClient>[] = [
	{ value: DownloadClient.Internal, label: 'Internal downloader' },
	{ value: DownloadClient.Aria2c, label: 'Aria2c' },
	{ value: DownloadClient.Symlink, label: 'Symlink downloader' },
	{ value: DownloadClient.DownloadStation, label: 'Synology Download Station' }
];

export const hostDownloadActionOptions: readonly Option<TorrentHostDownloadAction>[] = [
	{ value: TorrentHostDownloadAction.DownloadAll, label: 'Download all files to host' },
	{ value: TorrentHostDownloadAction.DownloadNone, label: "Don't download any files to host" }
];

export const downloadActionOptions: readonly Option<TorrentDownloadAction>[] = [
	{ value: TorrentDownloadAction.DownloadAll, label: 'Download all files' },
	{ value: TorrentDownloadAction.DownloadAvailableFiles, label: 'Download all available files' },
	{ value: TorrentDownloadAction.DownloadManual, label: 'Manually select files' }
];

export const finishedActionOptions: readonly Option<TorrentFinishedAction>[] = [
	{ value: TorrentFinishedAction.None, label: 'Do nothing' },
	{ value: TorrentFinishedAction.RemoveAllTorrents, label: 'Remove from provider and client' },
	{ value: TorrentFinishedAction.RemoveProvider, label: 'Remove from provider' },
	{ value: TorrentFinishedAction.RemoveClient, label: 'Remove from client' }
];

const providerStatusLabels: Record<TorrentStatus, string> = {
	[TorrentStatus.Queued]: 'Queued',
	[TorrentStatus.Processing]: 'Processing',
	[TorrentStatus.WaitingForFileSelection]: 'Waiting for file selection',
	[TorrentStatus.Downloading]: 'Downloading',
	[TorrentStatus.Finished]: 'Finished',
	[TorrentStatus.Uploading]: 'Uploading',
	[TorrentStatus.Error]: 'Error'
};

/** Label of the option with `value`, or an empty string when unknown. */
export function optionLabel<T extends number>(
	options: readonly Option<T>[],
	value: T | null | undefined
): string {
	return options.find((option) => option.value === value)?.label ?? '';
}

/** Label of a provider-side torrent status. */
export function providerStatusLabel(status: TorrentStatus | null | undefined): string {
	return status == null ? 'Unknown' : (providerStatusLabels[status] ?? 'Unknown');
}

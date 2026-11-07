<script lang="ts">
	import { errorMessage, torrents as torrentsApi } from '#lib/api/index.js';
	import type { Download, Torrent } from '#lib/api/types.js';
	import ConfirmDialog from '#lib/components/confirm-dialog.svelte';
	import DeleteTorrentDialog from '#lib/components/delete-torrent-dialog.svelte';
	import ExternalLink from '#lib/components/external-link.svelte';
	import TorrentProgress from '#lib/components/torrent-progress.svelte';
	import TorrentSettingsDialog from '#lib/components/torrent-settings-dialog.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Card from '#lib/components/ui/card/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import * as Table from '#lib/components/ui/table/index.js';
	import * as Tabs from '#lib/components/ui/tabs/index.js';
	import { decodeUrl, formatDateTime, formatSize, formatSpeed } from '#lib/format.js';
	import {
		downloadActionOptions,
		downloadClientOptions,
		finishedActionOptions,
		hostDownloadActionOptions,
		optionLabel,
		providerStatusLabel
	} from '#lib/labels.js';
	import { Latest } from '#lib/latest.js';
	import { live } from '#lib/live.svelte.js';
	import { downloadStatusText } from '#lib/status.js';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import CheckIcon from '@lucide/svelte/icons/check';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import SlidersHorizontalIcon from '@lucide/svelte/icons/sliders-horizontal';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import XIcon from '@lucide/svelte/icons/x';
	import { untrack } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';

	let detail = $state<Torrent | null>(null);
	let loadError = $state<string | null>(null);
	let copied = $state(false);
	// Without clipboard access (plain HTTP) the magnet link is shown selected for manual copying.
	let copyFailure = $state<string | null>(null);
	let copiedTimer: ReturnType<typeof setTimeout> | undefined;
	let deleteOpen = $state(false);
	let retryOpen = $state(false);
	let settingsOpen = $state(false);
	let retryDownloadId = $state<string | null>(null);
	const expanded = new SvelteSet<string>();

	const torrentId = $derived(page.params.id ?? '');
	const loads = new Latest();
	// Files count the reload below was last made for, so a mismatch never reloads in a loop.
	let filesReloadedFor: number | null = null;

	function load(id: string) {
		// Moving to another torrent reuses this page; only the latest reply is shown.
		const current = loads.next();
		torrentsApi
			.get(id)
			.then((result) => current() && (detail = result))
			.catch((e: unknown) => current() && (loadError = errorMessage(e)));
	}

	$effect(() => {
		const id = torrentId;
		detail = null;
		loadError = null;
		filesReloadedFor = null;
		load(id);
	});

	const update = $derived(live.torrents?.find((t) => t.torrentId === torrentId));
	// Shown once the live list no longer has the torrent, e.g. after another client deleted it.
	const removed = $derived(detail != null && live.torrents != null && update == null);

	// The provider reports files some time after the torrent was added; live updates only
	// carry their count, so the detail is reloaded when it changes.
	$effect(() => {
		const count = update?.filesCount;

		if (detail && count != null && count !== detail.files.length && count !== filesReloadedFor) {
			filesReloadedFor = count;
			untrack(() => load(torrentId));
		}
	});

	// Live updates carry fresh state and downloads but no files or magnet.
	const torrent = $derived.by(() => {
		if (!detail || !update) {
			return detail;
		}

		return {
			...detail,
			...update,
			files: detail.files,
			fileOrMagnet: detail.fileOrMagnet,
			downloads: update.downloads ?? detail.downloads
		};
	});

	async function copyMagnet(magnet: string) {
		copyFailure = null;
		clearTimeout(copiedTimer);

		try {
			if (!('clipboard' in navigator)) {
				throw new Error('The browser only allows copying on HTTPS or localhost.');
			}
			await navigator.clipboard.writeText(magnet);
			copied = true;
			copiedTimer = setTimeout(() => (copied = false), 2000);
		} catch (e) {
			copied = false;
			copyFailure = errorMessage(e);
		}
	}

	function selectAll(input: HTMLInputElement) {
		input.focus();
		input.select();
	}

	function downloadTorrentFile(current: Torrent) {
		const bytes = Uint8Array.from(atob(current.fileOrMagnet ?? ''), (c) => c.charCodeAt(0));
		const url = URL.createObjectURL(new Blob([bytes], { type: 'application/x-bittorrent' }));
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${current.rdName ?? current.hash}.torrent`;
		anchor.click();
		// Some browsers start the download after `click()` returns; revoking at once can cancel it.
		setTimeout(() => URL.revokeObjectURL(url), 60_000);
	}

	function toggle(download: Download) {
		if (expanded.has(download.downloadId)) {
			expanded.delete(download.downloadId);
		} else {
			expanded.add(download.downloadId);
		}
	}

	const minutes = (value: number) => `${value} ${value === 1 ? 'minute' : 'minutes'}`;
</script>

<svelte:head>
	<title>{torrent?.rdName ?? 'Torrent'} · RD Client</title>
</svelte:head>

{#if loadError}
	<Alert.Root variant="destructive">
		<CircleAlertIcon />
		<Alert.Title>Could not load the torrent</Alert.Title>
		<Alert.Description>{loadError}</Alert.Description>
	</Alert.Root>
{:else if !torrent}
	<div class="flex justify-center py-16"><Spinner class="size-6 text-muted-foreground" /></div>
{:else}
	{#if removed}
		<Alert.Root variant="destructive">
			<CircleAlertIcon />
			<Alert.Title>This torrent no longer exists</Alert.Title>
			<Alert.Description>
				<p>
					It was deleted, possibly from another client. <a
						href={resolve('')}
						class="underline underline-offset-4">Back to the torrents</a
					>
				</p>
			</Alert.Description>
		</Alert.Root>
	{/if}
	<div class="flex flex-wrap items-start gap-3">
		<div class="mr-auto grid min-w-0 gap-2">
			<h1 class="text-xl font-semibold tracking-tight break-all">
				{torrent.rdName ?? torrent.hash}
			</h1>
			<TorrentProgress {torrent} />
		</div>
		<div class="flex flex-wrap gap-2">
			<Button size="sm" variant="outline" onclick={() => (settingsOpen = true)}>
				<SlidersHorizontalIcon />
				Change settings
			</Button>
			<Button size="sm" variant="outline" onclick={() => (retryOpen = true)}>
				<RotateCcwIcon />
				Retry
			</Button>
			<Button size="sm" variant="destructive" onclick={() => (deleteOpen = true)}>
				<Trash2Icon />
				Delete
			</Button>
		</div>
	</div>

	<Tabs.Root value="general">
		<Tabs.List>
			<Tabs.Trigger value="general">General</Tabs.Trigger>
			<Tabs.Trigger value="files">Files ({torrent.files.length})</Tabs.Trigger>
			<Tabs.Trigger value="downloads">Downloads ({torrent.downloads.length})</Tabs.Trigger>
		</Tabs.List>

		<Tabs.Content value="general" class="grid gap-4 lg:grid-cols-2">
			<Card.Root>
				<Card.Header><Card.Title>Torrent</Card.Title></Card.Header>
				<Card.Content>
					<dl class="grid gap-3 text-sm sm:grid-cols-[auto_1fr] sm:gap-x-6">
						{@render item('Hash', torrent.hash)}
						{@render item('Retry count', `${torrent.retryCount} / ${torrent.torrentRetryAttempts}`)}
						{@render item('Priority', torrent.priority ? String(torrent.priority) : '')}
						{@render item('Category', torrent.category || '(no category)')}
						{@render item('Downloader', optionLabel(downloadClientOptions, torrent.downloadClient))}
						{@render item(
							'Post download action',
							optionLabel(hostDownloadActionOptions, torrent.hostDownloadAction)
						)}
						{@render item(
							'File selection',
							optionLabel(downloadActionOptions, torrent.downloadAction)
						)}
						{@render item(
							'Finished action',
							optionLabel(finishedActionOptions, torrent.finishedAction)
						)}
						{@render item('Finished action delay', minutes(torrent.finishedActionDelay ?? 0))}
						{@render item('Minimum file size', `${torrent.downloadMinSize} MB`)}
						{@render item('Include files', torrent.includeRegex ?? '')}
						{@render item('Exclude files', torrent.excludeRegex ?? '')}
						<dt class="text-muted-foreground">{torrent.isFile ? 'Torrent file' : 'Magnet link'}</dt>
						<dd>
							{#if torrent.fileOrMagnet}
								{#if torrent.isFile}
									<Button
										size="sm"
										variant="link"
										class="h-auto p-0"
										onclick={() => downloadTorrentFile(torrent)}
									>
										Download torrent file
									</Button>
								{:else}
									<Button
										size="sm"
										variant="link"
										class="h-auto p-0"
										onclick={() => copyMagnet(torrent.fileOrMagnet ?? '')}
									>
										{copied ? 'Copied to clipboard' : 'Copy magnet link'}
									</Button>
									{#if copyFailure}
										<p class="mt-1 text-destructive">Could not copy: {copyFailure} Copy it here:</p>
										<Input
											readonly
											aria-label="Magnet link"
											value={torrent.fileOrMagnet}
											class="mt-1"
											{@attach selectAll}
										/>
									{/if}
								{/if}
							{/if}
						</dd>
						{@render item('Added', formatDateTime(torrent.added))}
						{@render item('Files selected', formatDateTime(torrent.filesSelected) || '(not yet)')}
						{@render item('Completed', formatDateTime(torrent.completed) || '(not yet)')}
					</dl>
				</Card.Content>
			</Card.Root>
			<Card.Root>
				<Card.Header><Card.Title>Debrid provider</Card.Title></Card.Header>
				<Card.Content>
					<dl class="grid gap-3 text-sm sm:grid-cols-[auto_1fr] sm:gap-x-6">
						{@render item('ID', torrent.rdId ?? '')}
						{@render item('Name', torrent.rdName ?? '')}
						{@render item('Size', formatSize(torrent.rdSize))}
						{@render item('Host', torrent.rdHost ?? '')}
						{@render item('Split', torrent.rdSplit != null ? String(torrent.rdSplit) : '')}
						{@render item('Progress', `${torrent.rdProgress ?? 0}%`)}
						{@render item(
							'Status',
							`${providerStatusLabel(torrent.rdStatus)}${torrent.rdStatusRaw ? ` (${torrent.rdStatusRaw})` : ''}`
						)}
						{@render item('Added', formatDateTime(torrent.rdAdded))}
						{@render item('Ended', formatDateTime(torrent.rdEnded))}
						{@render item('Speed', formatSpeed(torrent.rdSpeed))}
						{@render item('Seeders', String(torrent.rdSeeders ?? 0))}
					</dl>
				</Card.Content>
			</Card.Root>
		</Tabs.Content>

		<Tabs.Content value="files">
			<div class="overflow-hidden rounded-xl border">
				<Table.Root>
					<Table.Header>
						<Table.Row>
							<Table.Head class="w-16">ID</Table.Head>
							<Table.Head>Path</Table.Head>
							<Table.Head class="text-right">Size</Table.Head>
							<Table.Head class="w-24 text-center">Selected</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each torrent.files as file (file.id)}
							<Table.Row>
								<Table.Cell class="tabular-nums">{file.id}</Table.Cell>
								<Table.Cell class="break-all whitespace-normal">{file.path}</Table.Cell>
								<Table.Cell class="text-right tabular-nums">{formatSize(file.bytes)}</Table.Cell>
								<Table.Cell class="text-center">
									{#if file.selected}
										<CheckIcon class="inline size-4 text-green-600" aria-label="Selected" />
									{:else}
										<XIcon class="inline size-4 text-muted-foreground" aria-label="Not selected" />
									{/if}
								</Table.Cell>
							</Table.Row>
						{:else}
							<Table.Row>
								<Table.Cell colspan={4} class="py-8 text-center text-muted-foreground">
									The provider has not reported any files yet.
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
		</Tabs.Content>

		<Tabs.Content value="downloads">
			<div class="overflow-hidden rounded-xl border">
				<Table.Root>
					<Table.Header>
						<Table.Row>
							<Table.Head class="w-8"></Table.Head>
							<Table.Head>Link</Table.Head>
							<Table.Head class="text-right">Size</Table.Head>
							<Table.Head>Status</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each torrent.downloads as download (download.downloadId)}
							<!-- The whole row toggles for pointer users; the button serves keyboards and screen readers. -->
							<Table.Row class="cursor-pointer" onclick={() => toggle(download)}>
								<Table.Cell>
									<Button
										variant="ghost"
										size="icon-sm"
										aria-expanded={expanded.has(download.downloadId)}
										aria-label="Show details"
										onclick={(event: MouseEvent) => {
											event.stopPropagation();
											toggle(download);
										}}
									>
										{#if expanded.has(download.downloadId)}
											<ChevronDownIcon />
										{:else}
											<ChevronRightIcon />
										{/if}
									</Button>
								</Table.Cell>
								<Table.Cell class="break-all whitespace-normal">
									{decodeUrl(download.link ?? download.path)}
								</Table.Cell>
								<Table.Cell class="text-right tabular-nums"
									>{download.size != null ? formatSize(download.size) : ''}</Table.Cell
								>
								<Table.Cell class={download.error ? 'text-destructive' : ''}>
									{downloadStatusText(download)}
								</Table.Cell>
							</Table.Row>
							{#if expanded.has(download.downloadId)}
								<Table.Row class="hover:bg-transparent">
									<Table.Cell></Table.Cell>
									<Table.Cell colspan={3} class="whitespace-normal">
										<div class="grid gap-4 py-2 lg:grid-cols-2">
											<dl class="grid gap-2 text-sm sm:grid-cols-[auto_1fr] sm:gap-x-6">
												{#if download.error}{@render item('Error', download.error)}{/if}
												<dt class="text-muted-foreground">Unrestricted link</dt>
												<dd class="break-all">
													{#if download.link}
														<ExternalLink href={download.link}
															>{decodeUrl(download.link)}</ExternalLink
														>
													{/if}
												</dd>
												{@render item('Provider link', download.path)}
												{#if download.downloadStarted && !download.downloadFinished && !download.completed}
													{@render item(
														'Transferred',
														`${formatSize(download.bytesDone)} / ${formatSize(download.bytesTotal)} (${formatSpeed(download.speed)})`
													)}
												{/if}
												{@render item(
													'Retry count',
													`${download.retryCount} / ${torrent.downloadRetryAttempts}`
												)}
											</dl>
											<dl class="grid gap-2 text-sm sm:grid-cols-[auto_1fr] sm:gap-x-6">
												{@render item('Added', formatDateTime(download.added) || '(not yet)')}
												{@render item(
													'Download queued',
													formatDateTime(download.downloadQueued) || '(not yet)'
												)}
												{@render item(
													'Download started',
													formatDateTime(download.downloadStarted) || '(not yet)'
												)}
												{@render item(
													'Download finished',
													formatDateTime(download.downloadFinished) || '(not yet)'
												)}
												{@render item(
													'Unpacking queued',
													formatDateTime(download.unpackingQueued) || '(not yet)'
												)}
												{@render item(
													'Unpacking started',
													formatDateTime(download.unpackingStarted) || '(not yet)'
												)}
												{@render item(
													'Unpacking finished',
													formatDateTime(download.unpackingFinished) || '(not yet)'
												)}
												{@render item(
													'Completed',
													formatDateTime(download.completed) || '(not yet)'
												)}
											</dl>
										</div>
										<Button
											size="sm"
											variant="outline"
											onclick={() => (retryDownloadId = download.downloadId)}
										>
											<RotateCcwIcon />
											Retry download
										</Button>
									</Table.Cell>
								</Table.Row>
							{/if}
						{:else}
							<Table.Row>
								<Table.Cell colspan={4} class="py-8 text-center text-muted-foreground">
									No downloads yet.
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
		</Tabs.Content>
	</Tabs.Root>

	<DeleteTorrentDialog
		bind:open={deleteOpen}
		torrentIds={[torrent.torrentId]}
		onDeleted={() => goto(resolve(''))}
	/>

	<ConfirmDialog
		bind:open={retryOpen}
		title="Retry torrent"
		confirmLabel="Retry"
		errorTitle="Could not retry"
		action={() => torrentsApi.retry(torrentId)}
		onDone={() => goto(resolve(''))}
	>
		<p>
			This deletes the torrent data and every local download, then adds the original magnet link or
			torrent file to the debrid provider again.
		</p>
	</ConfirmDialog>

	<ConfirmDialog
		bind:open={() => retryDownloadId != null, (open) => !open && (retryDownloadId = null)}
		title="Retry download"
		confirmLabel="Retry"
		errorTitle="Could not retry the download"
		action={() => torrentsApi.retryDownload(retryDownloadId ?? '')}
	>
		<p>This removes the local file and downloads it again from the debrid provider.</p>
	</ConfirmDialog>

	<TorrentSettingsDialog
		bind:open={settingsOpen}
		torrents={[torrent]}
		onSaved={([saved]) => (detail = { ...(detail ?? saved), ...saved })}
	/>
{/if}

{#snippet item(label: string, value: string)}
	<dt class="text-muted-foreground">{label}</dt>
	<dd class="break-all">{value}</dd>
{/snippet}

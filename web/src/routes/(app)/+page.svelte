<script lang="ts">
	import { errorMessage, torrents as torrentsApi } from '#lib/api/index.js';
	import type { Torrent } from '#lib/api/types.js';
	import ConfirmDialog from '#lib/components/confirm-dialog.svelte';
	import DeleteTorrentDialog from '#lib/components/delete-torrent-dialog.svelte';
	import TorrentProgress from '#lib/components/torrent-progress.svelte';
	import TorrentSettingsDialog from '#lib/components/torrent-settings-dialog.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import { Checkbox } from '#lib/components/ui/checkbox/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import * as Table from '#lib/components/ui/table/index.js';
	import { formatDateTime, formatNumber, formatSize } from '#lib/format.js';
	import { live } from '#lib/live.svelte.js';
	import {
		filterTorrents,
		loadTorrentSort,
		nextSort,
		saveTorrentSort,
		sortTorrents,
		type TorrentFilter,
		type TorrentSortField
	} from '#lib/torrent-list.js';
	import { cn } from '#lib/utils.js';
	import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
	import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import SearchIcon from '@lucide/svelte/icons/search';
	import SlidersHorizontalIcon from '@lucide/svelte/icons/sliders-horizontal';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import { onMount } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';

	let initial = $state<Torrent[] | null>(null);
	let loadError = $state<string | null>(null);
	let query = $state('');
	let filter = $state<TorrentFilter>('all');
	let sort = $state(loadTorrentSort(globalThis.localStorage));
	const selected = new SvelteSet<string>();

	let deleteOpen = $state(false);
	let retryOpen = $state(false);
	let settingsOpen = $state(false);

	const torrents = $derived(live.torrents ?? initial ?? []);
	const visible = $derived(sortTorrents(filterTorrents(torrents, query, filter), sort));
	const selectedTorrents = $derived(torrents.filter((t) => selected.has(t.torrentId)));
	const allVisibleSelected = $derived(
		visible.length > 0 && visible.every((t) => selected.has(t.torrentId))
	);
	const someVisibleSelected = $derived(visible.some((t) => selected.has(t.torrentId)));

	const filters: { value: TorrentFilter; label: string }[] = [
		{ value: 'all', label: 'All' },
		{ value: 'active', label: 'Active' },
		{ value: 'completed', label: 'Completed' },
		{ value: 'error', label: 'Errors' }
	];

	const columns: { field: TorrentSortField; label: string; class?: string }[] = [
		{ field: 'rdName', label: 'Name' },
		{ field: 'category', label: 'Category' },
		{ field: 'priority', label: 'Priority', class: 'text-right' },
		{ field: 'rdSeeders', label: 'Seeders', class: 'text-right' },
		{ field: 'filesCount', label: 'Files', class: 'text-right' },
		{ field: 'downloadsCount', label: 'Downloads', class: 'text-right' },
		{ field: 'rdSize', label: 'Size', class: 'text-right' },
		{ field: 'added', label: 'Added' },
		{ field: 'status', label: 'Status' }
	];

	// Torrents that disappeared (deleted, retried under a new id) leave the selection.
	$effect(() => {
		const ids = new Set(torrents.map((t) => t.torrentId));

		for (const id of selected) {
			if (!ids.has(id)) {
				selected.delete(id);
			}
		}
	});

	onMount(() => {
		torrentsApi
			.list()
			.then((result) => (initial = result ?? []))
			.catch((e: unknown) => (loadError = errorMessage(e)));
	});

	function sortBy(field: TorrentSortField) {
		sort = nextSort(sort, field);
		saveTorrentSort(globalThis.localStorage, sort);
	}

	function toggle(id: string, checked: boolean) {
		if (checked) {
			selected.add(id);
		} else {
			selected.delete(id);
		}
	}

	function toggleAll(checked: boolean) {
		for (const torrent of visible) {
			toggle(torrent.torrentId, checked);
		}
	}

	async function retrySelected() {
		const results = await Promise.allSettled(
			selectedTorrents.map((torrent) => torrentsApi.retry(torrent.torrentId))
		);
		const failure = results.find((result) => result.status === 'rejected');

		if (failure) {
			throw failure.reason;
		}
	}
</script>

<svelte:head>
	<title>Torrents · RD Client</title>
</svelte:head>

{#if loadError}
	<Alert.Root variant="destructive">
		<CircleAlertIcon />
		<Alert.Title>Could not load the torrents</Alert.Title>
		<Alert.Description>{loadError}. Refresh the page after fixing the problem.</Alert.Description>
	</Alert.Root>
{/if}

<div class="flex flex-wrap items-center gap-2">
	<h1 class="mr-auto text-xl font-semibold tracking-tight">Torrents</h1>
	<div class="relative w-full sm:w-64">
		<SearchIcon
			class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
		/>
		<Input class="pl-8" placeholder="Search torrents" aria-label="Search" bind:value={query} />
	</div>
	<div class="flex rounded-lg border p-0.5" role="group" aria-label="Filter">
		{#each filters as option (option.value)}
			<Button
				size="sm"
				variant={filter === option.value ? 'secondary' : 'ghost'}
				aria-pressed={filter === option.value}
				onclick={() => (filter = option.value)}
			>
				{option.label}
			</Button>
		{/each}
	</div>
</div>

<div class="flex flex-wrap items-center gap-2">
	<span class="text-sm text-muted-foreground">
		{selected.size > 0 ? `${selected.size} selected` : `${visible.length} of ${torrents.length}`}
	</span>
	<div class="ml-auto flex flex-wrap gap-2">
		<Button
			size="sm"
			variant="outline"
			disabled={selected.size === 0}
			onclick={() => (settingsOpen = true)}
		>
			<SlidersHorizontalIcon />
			Change settings
		</Button>
		<Button
			size="sm"
			variant="outline"
			disabled={selected.size === 0}
			onclick={() => (retryOpen = true)}
		>
			<RotateCcwIcon />
			Retry
		</Button>
		<Button
			size="sm"
			variant="destructive"
			disabled={selected.size === 0}
			onclick={() => (deleteOpen = true)}
		>
			<Trash2Icon />
			Delete
		</Button>
	</div>
</div>

{#if initial == null && live.torrents == null && !loadError}
	<div class="flex justify-center py-16"><Spinner class="size-6 text-muted-foreground" /></div>
{:else if visible.length === 0}
	<div class="rounded-xl border border-dashed py-16 text-center text-sm text-muted-foreground">
		{torrents.length === 0 ? 'No torrents yet.' : 'No torrents match the current filter.'}
	</div>
{:else}
	<div class="hidden overflow-hidden rounded-xl border md:block">
		<Table.Root>
			<Table.Header>
				<Table.Row>
					<Table.Head class="w-10">
						<Checkbox
							aria-label="Select all"
							checked={allVisibleSelected}
							indeterminate={someVisibleSelected && !allVisibleSelected}
							onCheckedChange={toggleAll}
						/>
					</Table.Head>
					{#each columns as column (column.field)}
						<Table.Head class={column.class}>
							<button
								type="button"
								class={cn(
									'inline-flex items-center gap-1 hover:text-foreground',
									column.class?.includes('text-right') && 'flex-row-reverse'
								)}
								onclick={() => sortBy(column.field)}
							>
								{column.label}
								{#if sort.field === column.field}
									{#if sort.direction === 'asc'}
										<ArrowUpIcon class="size-3.5" />
									{:else}
										<ArrowDownIcon class="size-3.5" />
									{/if}
								{/if}
							</button>
						</Table.Head>
					{/each}
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each visible as torrent (torrent.torrentId)}
					<Table.Row data-state={selected.has(torrent.torrentId) ? 'selected' : undefined}>
						<Table.Cell>
							<Checkbox
								aria-label="Select {torrent.rdName ?? torrent.hash}"
								checked={selected.has(torrent.torrentId)}
								onCheckedChange={(checked) => toggle(torrent.torrentId, checked)}
							/>
						</Table.Cell>
						<Table.Cell class="max-w-md font-medium break-all whitespace-normal">
							{torrent.rdName ?? torrent.hash}
						</Table.Cell>
						<Table.Cell>{torrent.category ?? ''}</Table.Cell>
						<Table.Cell class="text-right tabular-nums">{torrent.priority ?? ''}</Table.Cell>
						<Table.Cell class="text-right tabular-nums">{torrent.rdSeeders ?? ''}</Table.Cell>
						<Table.Cell class="text-right tabular-nums"
							>{formatNumber(torrent.filesCount)}</Table.Cell
						>
						<Table.Cell class="text-right tabular-nums">
							{formatNumber(torrent.downloadsCount)}
						</Table.Cell>
						<Table.Cell class="text-right tabular-nums">{formatSize(torrent.rdSize)}</Table.Cell>
						<Table.Cell class="text-muted-foreground">{formatDateTime(torrent.added)}</Table.Cell>
						<Table.Cell class="min-w-56 whitespace-normal">
							<TorrentProgress {torrent} />
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>

	<ul class="grid gap-2 md:hidden">
		{#each visible as torrent (torrent.torrentId)}
			<li
				class={cn(
					'flex gap-3 rounded-xl border p-3',
					selected.has(torrent.torrentId) && 'border-primary/50 bg-muted/50'
				)}
			>
				<Checkbox
					class="mt-0.5"
					aria-label="Select {torrent.rdName ?? torrent.hash}"
					checked={selected.has(torrent.torrentId)}
					onCheckedChange={(checked) => toggle(torrent.torrentId, checked)}
				/>
				<div class="grid min-w-0 flex-1 gap-2">
					<p class="text-sm font-medium break-all">{torrent.rdName ?? torrent.hash}</p>
					<TorrentProgress {torrent} />
					<p class="flex flex-wrap gap-x-3 text-xs text-muted-foreground">
						{#if torrent.category}<span>{torrent.category}</span>{/if}
						<span>{formatSize(torrent.rdSize)}</span>
						<span>{formatNumber(torrent.filesCount)} files</span>
						<span>{formatNumber(torrent.downloadsCount)} downloads</span>
						{#if torrent.rdSeeders != null}<span>{torrent.rdSeeders} seeders</span>{/if}
					</p>
				</div>
			</li>
		{/each}
	</ul>
{/if}

<DeleteTorrentDialog
	bind:open={deleteOpen}
	torrentIds={selectedTorrents.map((t) => t.torrentId)}
	onDeleted={() => selected.clear()}
/>

<ConfirmDialog
	bind:open={retryOpen}
	title={selected.size === 1 ? 'Retry torrent' : `Retry ${selected.size} torrents`}
	confirmLabel="Retry"
	errorTitle="Could not retry"
	action={retrySelected}
	onDone={() => selected.clear()}
>
	<p>
		This deletes the torrent data and every local download, then adds the original magnet link or
		torrent file to the debrid provider again.
	</p>
</ConfirmDialog>

<TorrentSettingsDialog
	bind:open={settingsOpen}
	torrents={selectedTorrents}
	onSaved={() => selected.clear()}
/>

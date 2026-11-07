<script lang="ts">
	import {
		allowedFinishedAction,
		guiDefaults,
		magnetFromHash,
		newTorrentErrors,
		parseCategories,
		type NumberField
	} from '#lib/add-torrent.js';
	import {
		errorMessage,
		settings as settingsApi,
		torrents as torrentsApi
	} from '#lib/api/index.js';
	import { DownloadClient, TorrentDownloadAction, type NewTorrent } from '#lib/api/types.js';
	import OptionSelect from '#lib/components/option-select.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Card from '#lib/components/ui/card/index.js';
	import { Checkbox } from '#lib/components/ui/checkbox/index.js';
	import * as Field from '#lib/components/ui/field/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import { Label } from '#lib/components/ui/label/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import { Textarea } from '#lib/components/ui/textarea/index.js';
	import { FileCheck } from '#lib/file-check.svelte.js';
	import { formatSize } from '#lib/format.js';
	import {
		downloadActionOptions,
		downloadClientOptions,
		finishedActionOptions,
		hostDownloadActionOptions
	} from '#lib/labels.js';
	import { Latest } from '#lib/latest.js';
	import { cn } from '#lib/utils.js';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import FileIcon from '@lucide/svelte/icons/file';
	import XIcon from '@lucide/svelte/icons/x';
	import { onMount } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';

	let magnet = $state(magnetFromHash(globalThis.location?.hash ?? '') ?? '');
	let files = $state<File[]>([]);
	let torrent = $state<NewTorrent | null>(null);
	let provider = $state<string | null>(null);
	let categories = $state<string[]>([]);

	const manual = new SvelteSet<string>();
	const fileCheck = new FileCheck((available) => {
		manual.clear();
		for (const file of available) manual.add(file.filename);
		void verifyRegex();
	});
	const regexCheck = new Latest();

	let includeError = $state('');
	let excludeError = $state('');
	let regexSelection = $state<Set<string> | null>(null);

	let saving = $state(false);
	let error = $state<string | null>(null);
	// Field errors show once a submit was attempted and then follow every edit.
	let validated = $state(false);
	const fieldErrors: Partial<Record<NumberField, string>> = $derived(
		validated && torrent ? newTorrentErrors(torrent) : {}
	);

	const fieldIds: Record<NumberField, string> = {
		priority: 'priority',
		downloadMinSize: 'min-size',
		finishedActionDelay: 'finished-delay',
		downloadRetryAttempts: 'download-retries',
		torrentRetryAttempts: 'torrent-retries',
		deleteOnError: 'delete-on-error',
		lifetime: 'lifetime'
	};

	const multiple = $derived(!magnet.trim() && files.length > 1);
	const finishedOptions = $derived(
		torrent?.downloadClient === DownloadClient.Symlink
			? finishedActionOptions.filter((o) => o.value === 0 || o.value === 3)
			: finishedActionOptions
	);

	onMount(() => {
		settingsApi
			.list()
			.then((settings) => {
				const list = settings ?? [];
				const defaults = guiDefaults(list);
				const providerSetting = list.find((s) => s.key === 'Provider:Provider');
				categories = parseCategories(list.find((s) => s.key === 'General:Categories')?.value);
				defaults.category =
					categories.find((c) => c.toLowerCase() === defaults.category?.toLowerCase()) ??
					defaults.category;
				provider = providerSetting?.enumValues?.[String(providerSetting.value)] ?? null;
				torrent = defaults;
				if (magnet) void checkFiles();
			})
			.catch((e: unknown) => (error = errorMessage(e)));
	});

	// The magnet field's blur starts a check right as the submit button is pressed; submit waits
	// for it instead of disabling the button, which would swallow that click.
	function checkFiles(): Promise<void> {
		const link = magnet.trim();
		const file = files.length === 1 ? files[0] : null;
		const source = link || (file ? `file:${file.name}:${file.size}` : '');

		return fileCheck.check(source, () => {
			if (link) return torrentsApi.checkFilesMagnet(link);
			return file ? torrentsApi.checkFiles(file) : Promise.resolve(null);
		});
	}

	async function verifyRegex() {
		const current = regexCheck.next();
		includeError = '';
		excludeError = '';
		regexSelection = null;
		const link = magnet.trim();
		if (!link || !torrent || (!torrent.includeRegex && !torrent.excludeRegex)) return;

		try {
			const result = await torrentsApi.verifyRegex(
				torrent.includeRegex,
				torrent.excludeRegex,
				link
			);
			if (!current()) return;
			includeError = result?.includeError ?? '';
			excludeError = result?.excludeError ?? '';
			regexSelection = new Set(result?.selectedFiles.map((f) => f.filename));
		} catch (e) {
			if (current()) includeError = errorMessage(e);
		}
	}

	function pickFiles(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		files = [...files, ...Array.from(input.files ?? [])];
		input.value = '';
		void checkFiles();
	}

	function removeFile(index: number) {
		files = files.filter((_, i) => i !== index);
		void checkFiles();
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!torrent || saving) return;
		validated = true;
		const invalid = Object.keys(newTorrentErrors(torrent))[0] as NumberField | undefined;
		if (invalid) {
			document.getElementById(fieldIds[invalid])?.focus();
			return;
		}
		saving = true;
		// A failed check only hides the file list; the torrent can still be added.
		await checkFiles();
		error = null;

		const settings: NewTorrent = { ...torrent, downloadManualFiles: null };
		settings.category = settings.category?.trim() || null;

		if (torrent.downloadAction === TorrentDownloadAction.DownloadManual && !multiple) {
			if (!fileCheck.available || manual.size === 0) {
				error = 'Select at least one file to download.';
				saving = false;
				return;
			}
			settings.downloadManualFiles = [...manual].join(',');
		}

		const link = magnet.trim();
		if (!link && files.length === 0) {
			error = 'Paste a magnet link or pick a torrent file.';
			saving = false;
			return;
		}

		try {
			if (link) {
				await torrentsApi.uploadMagnet(link, settings);
			} else {
				const failures: string[] = [];
				for (const file of files) {
					await torrentsApi
						.uploadFile(file, settings)
						.catch((e: unknown) => failures.push(`${file.name} (${errorMessage(e)})`));
				}
				if (failures.length > 0) {
					throw new Error(
						`${failures.length} of ${files.length} files failed: ${failures.join('; ')}`
					);
				}
			}
			await goto(resolve(''));
		} catch (e) {
			error = errorMessage(e);
			saving = false;
		}
	}
</script>

<svelte:head>
	<title>Add torrent · RD Client</title>
</svelte:head>

<h1 class="text-xl font-semibold tracking-tight">Add torrent</h1>

{#if !torrent}
	{#if error}
		{@render failure()}
	{:else}
		<div class="flex justify-center py-16"><Spinner class="size-6 text-muted-foreground" /></div>
	{/if}
{:else}
	<form class="grid gap-4 lg:grid-cols-[1fr_22rem]" onsubmit={submit}>
		<div class="grid content-start gap-4">
			<Card.Root>
				<Card.Content>
					<Field.Group>
						<Field.Field>
							<Field.Label for="magnet">Magnet link</Field.Label>
							<Textarea
								id="magnet"
								rows={3}
								placeholder="magnet:?xt=urn:btih:…"
								disabled={saving || files.length > 0}
								bind:value={magnet}
								onblur={checkFiles}
								onpaste={() => setTimeout(checkFiles, 50)}
							/>
						</Field.Field>
						<Field.Field>
							<Field.Label for="files">Or torrent files</Field.Label>
							<Input
								id="files"
								type="file"
								accept=".torrent,application/x-bittorrent"
								multiple
								disabled={saving || magnet.trim().length > 0}
								onchange={pickFiles}
							/>
							{#if files.length > 0}
								<ul class="grid max-h-48 gap-1 overflow-y-auto rounded-lg border p-1 text-sm">
									{#each files as file, index (index)}
										<li class="flex items-center gap-2 rounded px-2 py-1 hover:bg-muted">
											<FileIcon class="size-4 shrink-0 text-muted-foreground" />
											<span class="min-w-0 flex-1 truncate">{file.name}</span>
											<Button
												variant="ghost"
												size="icon-sm"
												aria-label="Remove {file.name}"
												disabled={saving}
												onclick={() => removeFile(index)}
											>
												<XIcon />
											</Button>
										</li>
									{/each}
								</ul>
							{/if}
						</Field.Field>
					</Field.Group>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<Card.Title>Files</Card.Title>
					<Card.Description>
						Files your debrid provider reports for this torrent. More files may exist than shown.
					</Card.Description>
				</Card.Header>
				<Card.Content class="grid gap-4">
					<Field.Field>
						<Field.Label for="download-action">File selection</Field.Label>
						<OptionSelect
							id="download-action"
							options={downloadActionOptions}
							bind:value={torrent.downloadAction}
						/>
						{#if multiple && torrent.downloadAction === TorrentDownloadAction.DownloadManual}
							<Field.Description>
								Manual selection does not apply when adding several torrent files; all files are
								downloaded.
							</Field.Description>
						{/if}
					</Field.Field>
					{#if fileCheck.checking}
						<div class="flex items-center gap-2 text-sm text-muted-foreground">
							<Spinner /> Checking the provider…
						</div>
					{:else if fileCheck.available && fileCheck.available.length > 0}
						<ul class="grid max-h-96 gap-1 overflow-y-auto rounded-lg border p-1 text-sm">
							{#each fileCheck.available as file (file.filename)}
								{@const excluded = regexSelection != null && !regexSelection.has(file.filename)}
								<li class="rounded px-2 py-1 hover:bg-muted">
									{#if torrent.downloadAction === TorrentDownloadAction.DownloadManual}
										<Label class="flex items-center gap-2 font-normal">
											<Checkbox
												checked={manual.has(file.filename)}
												onCheckedChange={(checked) =>
													checked ? manual.add(file.filename) : manual.delete(file.filename)}
											/>
											<span class="min-w-0 flex-1 break-all">{file.filename}</span>
											<span class="text-muted-foreground">{formatSize(file.filesize)}</span>
										</Label>
									{:else}
										<div class={cn('flex gap-2', excluded && 'text-muted-foreground line-through')}>
											<span class="min-w-0 flex-1 break-all">{file.filename}</span>
											{#if file.filesize > 0}
												<span class="text-muted-foreground">{formatSize(file.filesize)}</span>
											{/if}
										</div>
									{/if}
								</li>
							{/each}
						</ul>
					{:else if fileCheck.available}
						<p class="text-sm text-muted-foreground">
							The provider has no files for this torrent yet.
						</p>
					{:else}
						<p class="text-sm text-muted-foreground">
							Paste a magnet link or pick a single torrent file to check its files.
						</p>
					{/if}
				</Card.Content>
			</Card.Root>

			{#if error || fileCheck.error}{@render failure()}{/if}

			<div>
				<Button type="submit" disabled={saving}>
					{#if saving}<Spinner />{/if}
					Add torrent
				</Button>
			</div>
		</div>

		<Card.Root class="content-start">
			<Card.Header><Card.Title>Settings</Card.Title></Card.Header>
			<Card.Content>
				<Field.Group class="gap-5">
					<Field.Field>
						<Field.Label for="downloader">Downloader</Field.Label>
						<OptionSelect
							id="downloader"
							options={downloadClientOptions}
							bind:value={torrent.downloadClient}
							onValueChange={() =>
								torrent && (torrent.finishedAction = allowedFinishedAction(torrent))}
						/>
					</Field.Field>
					<Field.Field>
						<Field.Label for="host-action">Post download action</Field.Label>
						<OptionSelect
							id="host-action"
							options={hostDownloadActionOptions}
							bind:value={torrent.hostDownloadAction}
						/>
					</Field.Field>
					<Field.Field>
						<Field.Label for="category">Category</Field.Label>
						<Input id="category" list="category-options" bind:value={torrent.category} />
						<datalist id="category-options">
							{#each categories as category (category)}<option value={category}></option>{/each}
						</datalist>
						<Field.Description>Becomes a sub-folder of the download path.</Field.Description>
					</Field.Field>
					<Field.Field data-invalid={fieldErrors.priority ? true : undefined}>
						<Field.Label for="priority">Priority</Field.Label>
						<Input
							id="priority"
							type="number"
							min="0"
							step="1"
							aria-invalid={fieldErrors.priority ? true : undefined}
							bind:value={torrent.priority}
						/>
						<Field.Description>1 is the highest. Empty means lowest.</Field.Description>
						{@render numberError('priority')}
					</Field.Field>
					<Field.Field data-invalid={fieldErrors.downloadMinSize ? true : undefined}>
						<Field.Label for="min-size">Minimum file size (MB)</Field.Label>
						<Input
							id="min-size"
							type="number"
							min="0"
							step="1"
							aria-invalid={fieldErrors.downloadMinSize ? true : undefined}
							bind:value={torrent.downloadMinSize}
						/>
						{@render numberError('downloadMinSize')}
						{#if provider === 'AllDebrid'}
							<Field.Description>
								AllDebrid may group small files into one archive, so smaller files can still be
								downloaded.
							</Field.Description>
						{/if}
					</Field.Field>
					<Field.Field data-invalid={includeError ? true : undefined}>
						<Field.Label for="include">Include files (regular expression)</Field.Label>
						<Input id="include" bind:value={torrent.includeRegex} onblur={verifyRegex} />
						<Field.Description
							>Only download matching files. Use this or exclude, not both.</Field.Description
						>
						{#if includeError}<Field.Error>{includeError}</Field.Error>{/if}
					</Field.Field>
					<Field.Field data-invalid={excludeError ? true : undefined}>
						<Field.Label for="exclude">Exclude files (regular expression)</Field.Label>
						<Input id="exclude" bind:value={torrent.excludeRegex} onblur={verifyRegex} />
						<Field.Description>Skip matching files.</Field.Description>
						{#if excludeError}<Field.Error>{excludeError}</Field.Error>{/if}
					</Field.Field>
					<Field.Field>
						<Field.Label for="finished-action">Finished action</Field.Label>
						<OptionSelect
							id="finished-action"
							options={finishedOptions}
							bind:value={torrent.finishedAction}
						/>
					</Field.Field>
					<Field.Field data-invalid={fieldErrors.finishedActionDelay ? true : undefined}>
						<Field.Label for="finished-delay">Finished action delay (minutes)</Field.Label>
						<Input
							id="finished-delay"
							type="number"
							min="0"
							step="1"
							aria-invalid={fieldErrors.finishedActionDelay ? true : undefined}
							bind:value={torrent.finishedActionDelay}
						/>
						{@render numberError('finishedActionDelay')}
					</Field.Field>
					<Field.Field data-invalid={fieldErrors.downloadRetryAttempts ? true : undefined}>
						<Field.Label for="download-retries">Automatic download retries</Field.Label>
						<Input
							id="download-retries"
							type="number"
							min="0"
							step="1"
							aria-invalid={fieldErrors.downloadRetryAttempts ? true : undefined}
							bind:value={torrent.downloadRetryAttempts}
						/>
						{@render numberError('downloadRetryAttempts')}
					</Field.Field>
					<Field.Field data-invalid={fieldErrors.torrentRetryAttempts ? true : undefined}>
						<Field.Label for="torrent-retries">Automatic torrent retries</Field.Label>
						<Input
							id="torrent-retries"
							type="number"
							min="0"
							step="1"
							aria-invalid={fieldErrors.torrentRetryAttempts ? true : undefined}
							bind:value={torrent.torrentRetryAttempts}
						/>
						{@render numberError('torrentRetryAttempts')}
					</Field.Field>
					<Field.Field data-invalid={fieldErrors.deleteOnError ? true : undefined}>
						<Field.Label for="delete-on-error">Delete when in error (minutes)</Field.Label>
						<Input
							id="delete-on-error"
							type="number"
							min="0"
							step="1"
							aria-invalid={fieldErrors.deleteOnError ? true : undefined}
							bind:value={torrent.deleteOnError}
						/>
						<Field.Description>0 disables it.</Field.Description>
						{@render numberError('deleteOnError')}
					</Field.Field>
					<Field.Field data-invalid={fieldErrors.lifetime ? true : undefined}>
						<Field.Label for="lifetime">Maximum lifetime (minutes)</Field.Label>
						<Input
							id="lifetime"
							type="number"
							min="0"
							step="1"
							aria-invalid={fieldErrors.lifetime ? true : undefined}
							bind:value={torrent.lifetime}
						/>
						<Field.Description>0 disables it.</Field.Description>
						{@render numberError('lifetime')}
					</Field.Field>
				</Field.Group>
			</Card.Content>
		</Card.Root>
	</form>
{/if}

{#snippet failure()}
	<Alert.Root variant="destructive">
		<CircleAlertIcon />
		<Alert.Title>Could not add the torrent</Alert.Title>
		<Alert.Description>{error ?? fileCheck.error}</Alert.Description>
	</Alert.Root>
{/snippet}

{#snippet numberError(field: NumberField)}
	{#if fieldErrors[field]}<Field.Error>{fieldErrors[field]}</Field.Error>{/if}
{/snippet}

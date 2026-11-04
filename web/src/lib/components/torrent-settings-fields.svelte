<script lang="ts">
	import OptionSelect from '#lib/components/option-select.svelte';
	import * as Field from '#lib/components/ui/field/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import { downloadClientOptions, hostDownloadActionOptions } from '#lib/labels.js';
	import type { TorrentSettingsDraft } from '#lib/torrent-settings.js';

	let {
		draft = $bindable(),
		mixedPlaceholder = false
	}: {
		draft: TorrentSettingsDraft;
		/** Show "Unchanged" for blank values (editing several torrents). */
		mixedPlaceholder?: boolean;
	} = $props();

	const placeholder = $derived(mixedPlaceholder ? 'Unchanged' : undefined);
</script>

<Field.Group class="gap-5">
	<Field.Field>
		<Field.Label for="ts-downloader">Downloader</Field.Label>
		<OptionSelect
			id="ts-downloader"
			options={downloadClientOptions}
			bind:value={draft.downloadClient}
			placeholder={placeholder ?? 'Select…'}
		/>
		<Field.Description>Used to download the files from the provider to your host.</Field.Description
		>
	</Field.Field>

	<Field.Field>
		<Field.Label for="ts-host-action">Post download action</Field.Label>
		<OptionSelect
			id="ts-host-action"
			options={hostDownloadActionOptions}
			bind:value={draft.hostDownloadAction}
			placeholder={placeholder ?? 'Select…'}
		/>
		<Field.Description>
			What to do once the provider has finished the torrent. Choose not to download if you only want
			the files on your debrid provider.
		</Field.Description>
	</Field.Field>

	<Field.Field>
		<Field.Label for="ts-category">Category</Field.Label>
		<Input id="ts-category" bind:value={draft.category} {placeholder} />
		<Field.Description>The category becomes a sub-folder of the download path.</Field.Description>
	</Field.Field>

	<Field.Field>
		<Field.Label for="ts-priority">Priority</Field.Label>
		<Input
			id="ts-priority"
			type="number"
			step="1"
			min="0"
			bind:value={draft.priority}
			{placeholder}
		/>
		<Field.Description>
			1 is the highest priority. When empty the torrent gets the lowest priority.
		</Field.Description>
	</Field.Field>

	<div class="grid gap-5 sm:grid-cols-2">
		<Field.Field>
			<Field.Label for="ts-download-retries">Automatic download retries</Field.Label>
			<Input
				id="ts-download-retries"
				type="number"
				min="0"
				max="1000"
				step="1"
				bind:value={draft.downloadRetryAttempts}
				{placeholder}
			/>
			<Field.Description
				>Times a failed download is retried before it is marked failed.</Field.Description
			>
		</Field.Field>

		<Field.Field>
			<Field.Label for="ts-torrent-retries">Automatic torrent retries</Field.Label>
			<Input
				id="ts-torrent-retries"
				type="number"
				min="0"
				max="1000"
				step="1"
				bind:value={draft.torrentRetryAttempts}
				{placeholder}
			/>
			<Field.Description>
				Times the whole torrent is retried when a download keeps failing or the torrent errors.
			</Field.Description>
		</Field.Field>

		<Field.Field>
			<Field.Label for="ts-delete-on-error">Delete when in error (minutes)</Field.Label>
			<Input
				id="ts-delete-on-error"
				type="number"
				min="0"
				step="1"
				bind:value={draft.deleteOnError}
				{placeholder}
			/>
			<Field.Description>
				Delete the torrent from the provider and the client after this long in error. 0 disables it.
			</Field.Description>
		</Field.Field>

		<Field.Field>
			<Field.Label for="ts-lifetime">Maximum lifetime (minutes)</Field.Label>
			<Input
				id="ts-lifetime"
				type="number"
				min="0"
				step="1"
				bind:value={draft.lifetime}
				{placeholder}
			/>
			<Field.Description>
				Mark the torrent as failed after this long, unless it completed with downloads. 0 disables
				it.
			</Field.Description>
		</Field.Field>
	</div>
</Field.Group>

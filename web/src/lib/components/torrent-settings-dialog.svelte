<script lang="ts">
	import { errorMessage, torrents as torrentsApi } from '#lib/api/index.js';
	import type { Torrent } from '#lib/api/types.js';
	import TorrentSettingsFields from '#lib/components/torrent-settings-fields.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Dialog from '#lib/components/ui/dialog/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import {
		applySettings,
		commonSettings,
		draftErrors,
		settingsOf,
		type TorrentSettingsDraft
	} from '#lib/torrent-settings.js';
	import { untrack } from 'svelte';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';

	let {
		open = $bindable(false),
		torrents,
		onSaved
	}: {
		open: boolean;
		torrents: readonly Torrent[];
		onSaved?: (torrents: Torrent[]) => void;
	} = $props();

	let draft = $state<TorrentSettingsDraft>(commonSettings([]));
	let saving = $state(false);
	let error = $state<string | null>(null);
	let validated = $state(false);

	const errors = $derived(validated ? draftErrors(draft) : {});
	const bulk = $derived(torrents.length > 1);

	// Take a snapshot when the dialog opens; live updates must not reset the form.
	$effect(() => {
		if (open) {
			draft = untrack(() => commonSettings(torrents));
			error = null;
			validated = false;
		}
	});

	async function save() {
		validated = true;
		if (Object.keys(draftErrors(draft)).length > 0) return;
		saving = true;
		error = null;

		const updated = torrents.map((torrent) =>
			applySettings(torrent, draft, bulk ? 'bulk' : 'single')
		);
		const results = await Promise.allSettled(
			updated.map((torrent) => torrentsApi.update(torrent.torrentId, settingsOf(torrent)))
		);
		const failure = results.find((result) => result.status === 'rejected');

		saving = false;

		if (failure) {
			error = errorMessage(failure.reason);
			return;
		}

		open = false;
		onSaved?.(updated);
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="max-h-[90svh] overflow-y-auto sm:max-w-xl">
		<Dialog.Header>
			<Dialog.Title>
				{bulk ? `Change settings of ${torrents.length} torrents` : 'Change torrent settings'}
			</Dialog.Title>
			{#if bulk}
				<Dialog.Description>
					Blank fields have different values across the selected torrents and are left unchanged.
				</Dialog.Description>
			{/if}
		</Dialog.Header>

		<TorrentSettingsFields bind:draft mixedPlaceholder={bulk} {errors} />
		{#if error}
			<Alert.Root variant="destructive">
				<CircleAlertIcon />
				<Alert.Title>Could not save the settings</Alert.Title>
				<Alert.Description>{error}</Alert.Description>
			</Alert.Root>
		{/if}

		<Dialog.Footer>
			<Button variant="outline" onclick={() => (open = false)} disabled={saving}>Cancel</Button>
			<Button onclick={save} disabled={saving}>
				{#if saving}<Spinner />{/if}
				Save
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>

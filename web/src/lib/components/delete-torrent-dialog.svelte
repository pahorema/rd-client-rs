<script lang="ts">
	import { torrents as torrentsApi } from '#lib/api/index.js';
	import type { DeleteOptions } from '#lib/api/types.js';
	import ConfirmDialog from '#lib/components/confirm-dialog.svelte';
	import { Checkbox } from '#lib/components/ui/checkbox/index.js';
	import { Label } from '#lib/components/ui/label/index.js';
	import { Separator } from '#lib/components/ui/separator/index.js';

	let {
		open = $bindable(false),
		torrentIds,
		onDeleted
	}: {
		open: boolean;
		torrentIds: readonly string[];
		onDeleted?: () => void;
	} = $props();

	let options = $state<DeleteOptions>({
		deleteData: false,
		deleteRdTorrent: false,
		deleteLocalFiles: false
	});

	const choices: { key: keyof DeleteOptions; label: string }[] = [
		{ key: 'deleteData', label: 'Delete from the client' },
		{ key: 'deleteRdTorrent', label: 'Delete from the debrid provider' },
		{ key: 'deleteLocalFiles', label: 'Delete the downloaded files' }
	];

	const all = $derived(options.deleteData && options.deleteRdTorrent && options.deleteLocalFiles);
	const some = $derived(options.deleteData || options.deleteRdTorrent || options.deleteLocalFiles);
	const single = $derived(torrentIds.length === 1);

	$effect(() => {
		if (open) {
			options = { deleteData: false, deleteRdTorrent: false, deleteLocalFiles: false };
		}
	});

	function selectAll(checked: boolean) {
		options = { deleteData: checked, deleteRdTorrent: checked, deleteLocalFiles: checked };
	}

	async function remove() {
		const ids = [...torrentIds];
		const results = await Promise.allSettled(ids.map((id) => torrentsApi.delete(id, options)));
		const failure = results.find((result) => result.status === 'rejected');

		if (failure) {
			throw failure.reason;
		}
	}
</script>

<ConfirmDialog
	bind:open
	title={single ? 'Delete torrent' : `Delete ${torrentIds.length} torrents`}
	confirmLabel="Delete"
	confirmVariant="destructive"
	errorTitle="Could not delete"
	action={remove}
	onDone={onDeleted}
>
	<p>
		Choose what to delete for {single ? 'this torrent' : 'the selected torrents'}. Deleting a
		torrent from the debrid provider also removes it here.
	</p>
	<div class="grid gap-3">
		{#each choices as choice (choice.key)}
			<Label class="flex items-center gap-2 font-normal">
				<Checkbox bind:checked={options[choice.key]} />
				{choice.label}
			</Label>
		{/each}
		<Separator />
		<Label class="flex items-center gap-2 font-normal">
			<Checkbox checked={all} indeterminate={some && !all} onCheckedChange={selectAll} />
			Select all
		</Label>
	</div>
</ConfirmDialog>

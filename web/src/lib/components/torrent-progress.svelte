<script lang="ts">
	import type { Torrent } from '#lib/api/types.js';
	import { Progress } from '#lib/components/ui/progress/index.js';
	import { torrentProgress } from '#lib/status.js';
	import { torrentState } from '#lib/torrent-list.js';
	import { cn } from '#lib/utils.js';

	let { torrent }: { torrent: Torrent } = $props();

	const state = $derived(torrentState(torrent));
	const progress = $derived(torrentProgress(torrent));
</script>

<div class="grid gap-1.5">
	<span
		class={cn(
			'text-sm break-words',
			state === 'error' && 'text-destructive',
			state === 'completed' && 'text-green-700 dark:text-green-500'
		)}
	>
		{torrent.statusText}
	</span>
	{#if state === 'active'}
		<Progress value={progress} max={100} class="h-1.5" aria-label="Progress" />
	{/if}
</div>

<script lang="ts">
	import { errorMessage, settings as settingsApi } from '#lib/api/index.js';
	import { DownloadClient } from '#lib/api/types.js';
	import { buttonVariants } from '#lib/components/ui/button/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import * as Popover from '#lib/components/ui/popover/index.js';
	import { Slider } from '#lib/components/ui/slider/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import { page } from '$app/state';
	import GaugeIcon from '@lucide/svelte/icons/gauge';

	let visible = $state(false);
	let speed = $state(0);
	let max = $state(100);
	let saving = $state(false);
	let error = $state<string | null>(null);

	// Reload on every navigation so changes made on the settings page show up.
	$effect(() => {
		void page.route.id;
		if (saving) return;

		settingsApi
			.list()
			.then((settings) => {
				const value = (key: string) => Number(settings?.find((s) => s.key === key)?.value);
				visible = value('DownloadClient:Client') === DownloadClient.Internal;
				speed = Math.max(value('DownloadClient:MaxSpeed') || 0, 0);
				max = Math.max(value('DownloadClient:MaxSpeedSliderMax') || 100, speed, 1);
			})
			.catch(() => (visible = false));
	});

	async function save(value: number) {
		const rounded = Math.round(value);
		speed = Number.isFinite(rounded) && rounded > 0 ? Math.min(rounded, max) : 0;
		saving = true;
		error = null;

		try {
			await settingsApi.updateValue('DownloadClient:MaxSpeed', speed);
		} catch (e) {
			error = errorMessage(e);
		} finally {
			saving = false;
		}
	}
</script>

{#if visible}
	<Popover.Root>
		<Popover.Trigger
			class={buttonVariants({ variant: 'ghost', size: 'sm' })}
			title="Internal downloader speed limit (per download)"
		>
			<GaugeIcon />
			<span class="tabular-nums">{speed > 0 ? `${speed} MB/s` : 'Unlimited'}</span>
		</Popover.Trigger>
		<Popover.Content align="end" class="grid w-72 gap-3">
			<div class="flex items-center justify-between text-sm font-medium">
				Speed limit per download
				{#if saving}<Spinner />{/if}
			</div>
			<Slider
				type="single"
				min={0}
				{max}
				step={1}
				bind:value={speed}
				onValueCommit={save}
				aria-label="Speed limit in MB/s"
			/>
			<div class="flex items-center gap-2 text-sm">
				<Input
					type="number"
					min="0"
					{max}
					class="w-24"
					value={speed}
					aria-label="Speed limit in MB/s"
					onchange={(event) => save(Number(event.currentTarget.value))}
				/>
				MB/s
				<span class="ml-auto text-muted-foreground">0 = unlimited</span>
			</div>
			{#if error}<p class="text-sm text-destructive">{error}</p>{/if}
		</Popover.Content>
	</Popover.Root>
{/if}

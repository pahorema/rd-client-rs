<script lang="ts">
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { formatDateTime, parseTimestamp } from '#lib/format.js';
	import { live } from '#lib/live.svelte.js';
	import HardDriveIcon from '@lucide/svelte/icons/hard-drive';
	import TimerIcon from '@lucide/svelte/icons/timer';
	import { onMount } from 'svelte';

	let now = $state(Date.now());

	const resumeAt = $derived(parseTimestamp(live.rateLimit?.nextDequeueTime));
	const secondsLeft = $derived(resumeAt ? Math.ceil((resumeAt.getTime() - now) / 1000) : 0);

	onMount(() => {
		const timer = setInterval(() => (now = Date.now()), 1000);

		return () => clearInterval(timer);
	});

	const warning =
		'border-amber-500/40 bg-amber-50 text-amber-900 dark:bg-amber-950/40 dark:text-amber-200 *:data-[slot=alert-description]:text-amber-800 dark:*:data-[slot=alert-description]:text-amber-300';
</script>

{#if live.diskSpace?.isPaused}
	<Alert.Root class={warning}>
		<HardDriveIcon />
		<Alert.Title>Internal downloader paused: low disk space</Alert.Title>
		<Alert.Description>
			<p>
				Available: {live.diskSpace.availableSpaceGB} GB · Resumes at: {live.diskSpace.thresholdGB *
					2} GB · Last check: {formatDateTime(live.diskSpace.lastCheckTime)}
			</p>
		</Alert.Description>
	</Alert.Root>
{/if}

{#if resumeAt && secondsLeft > 0}
	<Alert.Root class={warning}>
		<TimerIcon />
		<Alert.Title>Debrid provider rate limit reached</Alert.Title>
		<Alert.Description>
			<p>
				Processing is paused until {resumeAt.toLocaleTimeString()} ({secondsLeft}
				{secondsLeft === 1 ? 'second' : 'seconds'} remaining).
			</p>
		</Alert.Description>
	</Alert.Root>
{/if}

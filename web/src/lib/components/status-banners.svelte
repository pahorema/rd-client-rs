<script lang="ts">
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { formatDateTime, parseTimestamp } from '#lib/format.js';
	import { live } from '#lib/live.svelte.js';
	import HardDriveIcon from '@lucide/svelte/icons/hard-drive';
	import TimerIcon from '@lucide/svelte/icons/timer';
	import WifiOffIcon from '@lucide/svelte/icons/wifi-off';
	import { onMount } from 'svelte';

	const mountedAt = Date.now();
	let now = $state(mountedAt);

	const resumeAt = $derived(parseTimestamp(live.rateLimit?.nextDequeueTime));
	const secondsLeft = $derived(resumeAt ? Math.ceil((resumeAt.getTime() - now) / 1000) : 0);
	// Live data freezes while the event stream reconnects. Opening it takes a moment, so only
	// a lost connection or one that is slow to open is reported.
	const disconnected = $derived(
		!live.connected && (live.torrents != null || now - mountedAt >= 3000)
	);

	onMount(() => {
		const timer = setInterval(() => (now = Date.now()), 1000);

		return () => clearInterval(timer);
	});

	const warning =
		'border-amber-500/40 bg-amber-50 text-amber-900 dark:bg-amber-950/40 dark:text-amber-200 *:data-[slot=alert-description]:text-amber-800 dark:*:data-[slot=alert-description]:text-amber-300';
</script>

{#if disconnected}
	<Alert.Root class={warning}>
		<WifiOffIcon />
		<Alert.Title>Reconnecting to the server…</Alert.Title>
		<Alert.Description>
			<p>Torrent states are not updated until the connection is back.</p>
		</Alert.Description>
	</Alert.Root>
{/if}
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

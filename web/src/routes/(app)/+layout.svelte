<script lang="ts">
	import { ApiError, auth, errorMessage } from '#lib/api/index.js';
	import Navbar from '#lib/components/navbar.svelte';
	import StatusBanners from '#lib/components/status-banners.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import { startLive } from '#lib/live.svelte.js';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import { onMount } from 'svelte';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	let ready = $state(false);
	let error = $state<string | null>(null);

	onMount(() => {
		let stopLive: (() => void) | undefined;
		let destroyed = false;

		auth
			.isLoggedIn()
			.then(() => {
				if (!destroyed) {
					ready = true;
					stopLive = startLive();
				}
			})
			.catch((e: unknown) => {
				// Missing setup or session already redirected to the right page.
				if (!(e instanceof ApiError && [401, 402, 403].includes(e.status))) {
					error = errorMessage(e);
				}
			});

		return () => {
			destroyed = true;
			stopLive?.();
		};
	});
</script>

{#if ready}
	<Navbar />
	<main class="mx-auto flex w-full max-w-7xl flex-col gap-4 px-4 py-6">
		<StatusBanners />
		{@render children()}
	</main>
{:else if error}
	<main class="mx-auto max-w-xl px-4 py-16">
		<Alert.Root variant="destructive">
			<CircleAlertIcon />
			<Alert.Title>Cannot reach the server</Alert.Title>
			<Alert.Description>{error}</Alert.Description>
		</Alert.Root>
	</main>
{:else}
	<div class="flex min-h-svh items-center justify-center">
		<Spinner class="size-6 text-muted-foreground" />
	</div>
{/if}

<script lang="ts">
	import { errorMessage } from '#lib/api/index.js';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Button, type ButtonVariant } from '#lib/components/ui/button/index.js';
	import * as Dialog from '#lib/components/ui/dialog/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import type { Snippet } from 'svelte';

	let {
		open = $bindable(false),
		title,
		confirmLabel,
		confirmVariant = 'default',
		errorTitle,
		action,
		onDone,
		children
	}: {
		open: boolean;
		title: string;
		confirmLabel: string;
		confirmVariant?: ButtonVariant;
		/** Heading shown above the server error. */
		errorTitle: string;
		/** Runs when confirmed; a rejection keeps the dialog open and shows the error. */
		action: () => Promise<unknown>;
		onDone?: () => void;
		children: Snippet;
	} = $props();

	let working = $state(false);
	let error = $state<string | null>(null);

	$effect(() => {
		if (open) {
			error = null;
		}
	});

	async function confirm() {
		working = true;
		error = null;

		try {
			await action();
			open = false;
			onDone?.();
		} catch (e) {
			error = errorMessage(e);
		} finally {
			working = false;
		}
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title>{title}</Dialog.Title>
		</Dialog.Header>

		<div class="grid gap-3 text-sm">
			{@render children()}
		</div>

		{#if error}
			<Alert.Root variant="destructive">
				<CircleAlertIcon />
				<Alert.Title>{errorTitle}</Alert.Title>
				<Alert.Description>{error}</Alert.Description>
			</Alert.Root>
		{/if}

		<Dialog.Footer>
			<Button variant="outline" onclick={() => (open = false)} disabled={working}>Cancel</Button>
			<Button variant={confirmVariant} onclick={confirm} disabled={working}>
				{#if working}<Spinner />{/if}
				{confirmLabel}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>

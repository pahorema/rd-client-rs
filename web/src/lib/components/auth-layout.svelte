<script lang="ts">
	import logo from '#lib/assets/logo.svg';
	import ThemeToggle from '#lib/components/theme-toggle.svelte';
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();
</script>

<div class="relative flex min-h-svh flex-col items-center justify-center bg-muted/40 px-4 py-10">
	<div class="absolute top-3 right-3">
		<ThemeToggle />
	</div>
	<div class="mb-6 flex items-center gap-3">
		<img src={logo} alt="" class="size-10" />
		<span class="text-2xl font-semibold tracking-tight">RD Client</span>
	</div>
	<div class="w-full max-w-md">
		{@render children()}
	</div>
</div>

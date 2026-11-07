<script lang="ts">
	import { externalHref } from '#lib/links.js';
	import { cn } from '#lib/utils.js';
	import type { Snippet } from 'svelte';

	let {
		href,
		class: className,
		children
	}: { href: string; class?: string; children: Snippet } = $props();

	const safeHref = $derived(externalHref(href));
</script>

{#if safeHref}
	<!-- Links to other sites never go through the app router. -->
	<!-- eslint-disable svelte/no-navigation-without-resolve -->
	<a
		href={safeHref}
		target="_blank"
		rel="noopener noreferrer"
		class={cn('text-primary underline-offset-4 hover:underline', className)}
	>
		{@render children()}
	</a>
	<!-- eslint-enable svelte/no-navigation-without-resolve -->
{:else}
	<span class={className}>{@render children()}</span>
{/if}

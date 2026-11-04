<script lang="ts">
	import type { Profile } from '#lib/api/types.js';
	import ExternalLink from '#lib/components/external-link.svelte';
	import { formatDate } from '#lib/format.js';
	import { providerByName } from '#lib/providers.js';
	import { cn } from '#lib/utils.js';
	import CrownIcon from '@lucide/svelte/icons/crown';

	let { profile }: { profile: Profile } = $props();

	const provider = $derived(providerByName(profile.provider));
	const text = $derived(
		profile.expiration
			? `Premium until ${formatDate(profile.expiration)}`
			: profile.premium
				? 'Premium: unlimited'
				: 'Not premium'
	);
</script>

<ExternalLink
	href={provider?.website ?? 'https://github.com/pahorema/rd-client-rs'}
	class={cn(
		'hidden items-center gap-1.5 rounded-md px-2 py-1 text-sm text-muted-foreground no-underline hover:bg-accent hover:text-foreground hover:no-underline lg:inline-flex',
		!profile.premium && !profile.expiration && 'text-destructive'
	)}
>
	<CrownIcon class="size-4" />
	{text}
</ExternalLink>

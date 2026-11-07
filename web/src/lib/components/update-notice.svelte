<script lang="ts">
	import type { Profile } from '#lib/api/types.js';
	import ExternalLink from '#lib/components/external-link.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { isNewerVersion } from '#lib/version.js';
	import ShieldAlertIcon from '@lucide/svelte/icons/shield-alert';
	import SparklesIcon from '@lucide/svelte/icons/sparkles';

	let { profile }: { profile: Profile | null } = $props();

	// Security notices are shown even when update notifications are disabled.
	const visible = $derived(
		profile != null &&
			!!profile.latestVersion &&
			isNewerVersion(profile.latestVersion, profile.currentVersion ?? '') &&
			(!profile.disableUpdateNotification || profile.isInsecure === true)
	);
</script>

{#if visible && profile}
	<Alert.Root
		variant={profile.isInsecure ? 'destructive' : 'default'}
		class={profile.isInsecure ? '' : 'border-primary/40'}
	>
		{#if profile.isInsecure}<ShieldAlertIcon />{:else}<SparklesIcon />{/if}
		<Alert.Title>
			{profile.isInsecure ? 'Your version has a known security issue' : 'An update is available'}
		</Alert.Title>
		<Alert.Description>
			<p>
				Version {profile.latestVersion} is available; you are running {profile.currentVersion}.
				<ExternalLink href="https://github.com/pahorema/rd-client-rs/releases/latest">
					Release notes
				</ExternalLink>
			</p>
		</Alert.Description>
	</Alert.Root>
{/if}

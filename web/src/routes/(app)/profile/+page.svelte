<script lang="ts">
	import { auth, errorMessage, settings as settingsApi } from '#lib/api/index.js';
	import type { Profile } from '#lib/api/types.js';
	import ExternalLink from '#lib/components/external-link.svelte';
	import UpdateNotice from '#lib/components/update-notice.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Badge } from '#lib/components/ui/badge/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Card from '#lib/components/ui/card/index.js';
	import * as Field from '#lib/components/ui/field/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import { formatDate } from '#lib/format.js';
	import { providerByName } from '#lib/providers.js';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
	import { onMount } from 'svelte';

	let profile = $state<Profile | null>(null);
	let profileError = $state<string | null>(null);
	let userName = $state('');
	let password = $state('');
	let saving = $state(false);
	let saved = $state(false);
	let saveError = $state<string | null>(null);

	const provider = $derived(providerByName(profile?.provider));

	onMount(() => {
		settingsApi
			.profile()
			.then((result) => (profile = result))
			.catch((e: unknown) => (profileError = errorMessage(e)));
	});

	async function save(event: SubmitEvent) {
		event.preventDefault();
		saving = true;
		saved = false;
		saveError = null;

		try {
			await auth.update(userName, password);
			saved = true;
			password = '';
		} catch (e) {
			saveError = errorMessage(e);
		} finally {
			saving = false;
		}
	}
</script>

<svelte:head>
	<title>Profile · RD Client</title>
</svelte:head>

<h1 class="text-xl font-semibold tracking-tight">Profile</h1>

<UpdateNotice {profile} />

<div class="grid gap-4 lg:grid-cols-2">
	<Card.Root>
		<Card.Header>
			<Card.Title>Debrid account</Card.Title>
		</Card.Header>
		<Card.Content>
			{#if profileError}
				<Alert.Root variant="destructive">
					<CircleAlertIcon />
					<Alert.Title>Could not load the provider account</Alert.Title>
					<Alert.Description>{profileError}</Alert.Description>
				</Alert.Root>
			{:else if !profile}
				<Spinner class="text-muted-foreground" />
			{:else}
				<dl class="grid gap-3 text-sm sm:grid-cols-[auto_1fr] sm:gap-x-6">
					<dt class="text-muted-foreground">Provider</dt>
					<dd>
						{#if provider}
							<ExternalLink href={provider.website}>{provider.label}</ExternalLink>
						{:else}
							{profile.provider ?? ''}
						{/if}
					</dd>
					<dt class="text-muted-foreground">Account</dt>
					<dd>{profile.userName ?? ''}</dd>
					<dt class="text-muted-foreground">Premium</dt>
					<dd>
						{#if profile.expiration}
							<Badge>Until {formatDate(profile.expiration)}</Badge>
						{:else if profile.premium}
							<Badge>Unlimited</Badge>
						{:else}
							<Badge variant="destructive">Not premium</Badge>
						{/if}
					</dd>
					<dt class="text-muted-foreground">Version</dt>
					<dd>
						{profile.currentVersion ?? ''}
						{#if profile.latestVersion}
							<span class="text-muted-foreground">(latest {profile.latestVersion})</span>
						{/if}
					</dd>
				</dl>
			{/if}
		</Card.Content>
	</Card.Root>

	<Card.Root>
		<Card.Header>
			<Card.Title>Sign-in credentials</Card.Title>
			<Card.Description
				>Change the username and password used to sign in to RD Client.</Card.Description
			>
		</Card.Header>
		<Card.Content>
			<form onsubmit={save}>
				<Field.Group>
					<Field.Field>
						<Field.Label for="userName">Username</Field.Label>
						<Input id="userName" autocomplete="username" required bind:value={userName} />
					</Field.Field>
					<Field.Field>
						<Field.Label for="password">New password</Field.Label>
						<Input
							id="password"
							type="password"
							autocomplete="new-password"
							required
							bind:value={password}
						/>
						<Field.Description>At least 5 characters, including a digit.</Field.Description>
					</Field.Field>
					{#if saveError}
						<Alert.Root variant="destructive">
							<CircleAlertIcon />
							<Alert.Title>Could not save</Alert.Title>
							<Alert.Description>{saveError}</Alert.Description>
						</Alert.Root>
					{:else if saved}
						<Alert.Root class="text-green-700 dark:text-green-500">
							<CircleCheckIcon />
							<Alert.Description class="text-current"
								>Your credentials have been updated.</Alert.Description
							>
						</Alert.Root>
					{/if}
					<div>
						<Button type="submit" disabled={saving}>
							{#if saving}<Spinner />{/if}
							Save
						</Button>
					</div>
				</Field.Group>
			</form>
		</Card.Content>
	</Card.Root>
</div>

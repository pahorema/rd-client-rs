<script lang="ts">
	import { ApiError, auth, errorMessage } from '#lib/api/index.js';
	import { Provider } from '#lib/api/types.js';
	import AuthLayout from '#lib/components/auth-layout.svelte';
	import ExternalLink from '#lib/components/external-link.svelte';
	import OptionSelect from '#lib/components/option-select.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Card from '#lib/components/ui/card/index.js';
	import * as Field from '#lib/components/ui/field/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import { providers } from '#lib/providers.js';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
	import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
	import { onMount } from 'svelte';

	// 0 while finding out where to resume: a signed-in visitor already created the account.
	let step = $state<0 | 1 | 2 | 3>(0);
	let userName = $state('');
	let password = $state('');
	let provider = $state<Provider>(Provider.RealDebrid);
	let token = $state('');
	let working = $state(false);
	let error = $state<string | null>(null);

	const providerOptions = providers.map((p) => ({ value: p.value, label: p.label }));
	const selectedProvider = $derived(providers.find((p) => p.value === provider));

	onMount(() => {
		auth
			.isLoggedIn()
			.then(() => (step = 2))
			// 402: no account yet. A 403 (account exists, signed out) goes to the login page.
			.catch(() => (step = 1));
	});

	async function run(action: () => Promise<unknown>, next: 2 | 3, refused: string) {
		working = true;
		error = null;

		try {
			await action();
			step = next;
		} catch (e) {
			error = e instanceof ApiError && e.status === 403 ? refused : errorMessage(e);
		} finally {
			working = false;
		}
	}

	function createAccount(event: SubmitEvent) {
		event.preventDefault();
		void run(
			() => auth.create(userName, password),
			2,
			'An account already exists. Sign in to finish the setup.'
		);
	}

	function connectProvider(event: SubmitEvent) {
		event.preventDefault();
		void run(
			() => auth.setupProvider(provider, token.trim()),
			3,
			'A debrid provider is already connected. You can change it on the Settings page.'
		);
	}
</script>

<svelte:head>
	<title>Setup · RD Client</title>
</svelte:head>

<AuthLayout>
	<Card.Root>
		{#if step === 0}
			<Card.Content class="flex justify-center py-8">
				<Spinner class="size-6 text-muted-foreground" />
			</Card.Content>
		{:else if step === 1}
			<Card.Header>
				<Card.Title>Welcome</Card.Title>
				<Card.Description>Create the account you will use to sign in to RD Client.</Card.Description
				>
			</Card.Header>
			<Card.Content>
				<form onsubmit={createAccount}>
					<Field.Group>
						<Field.Field>
							<Field.Label for="userName">Username</Field.Label>
							<Input id="userName" autocomplete="username" required bind:value={userName} />
						</Field.Field>
						<Field.Field>
							<Field.Label for="password">Password</Field.Label>
							<Input
								id="password"
								type="password"
								autocomplete="new-password"
								required
								bind:value={password}
							/>
							<Field.Description>At least 8 characters, including a digit.</Field.Description>
						</Field.Field>
						{@render failure()}
						<Button type="submit" disabled={working}>
							{#if working}<Spinner />{/if}
							Create account
						</Button>
					</Field.Group>
				</form>
			</Card.Content>
		{:else if step === 2}
			<Card.Header>
				<Card.Title>Connect your debrid provider</Card.Title>
				<Card.Description>
					A premium subscription with one of these providers is required to download torrents.
				</Card.Description>
			</Card.Header>
			<Card.Content>
				<ul class="mb-6 grid grid-cols-2 gap-2 text-sm">
					{#each providers as item (item.value)}
						<li>
							<ExternalLink href={item.website} class="inline-flex items-center gap-1">
								{item.label}
								<ExternalLinkIcon class="size-3" />
							</ExternalLink>
						</li>
					{/each}
				</ul>
				<form onsubmit={connectProvider}>
					<Field.Group>
						<Field.Field>
							<Field.Label for="provider">Provider</Field.Label>
							<OptionSelect id="provider" options={providerOptions} bind:value={provider} />
						</Field.Field>
						<Field.Field>
							<Field.Label for="token">API key</Field.Label>
							<Input id="token" autocomplete="off" required bind:value={token} />
							<Field.Description>
								{#if selectedProvider?.apiKeyUrl}
									You can find your API key at
									<ExternalLink href={selectedProvider.apiKeyUrl}>
										{selectedProvider.apiKeyUrl}
									</ExternalLink>.
								{:else}
									{selectedProvider?.apiKeyHelp}
								{/if}
							</Field.Description>
						</Field.Field>
						{@render failure()}
						<Button type="submit" disabled={working}>
							{#if working}<Spinner />{/if}
							Connect
						</Button>
					</Field.Group>
				</form>
			</Card.Content>
		{:else}
			<Card.Header>
				<Card.Title class="flex items-center gap-2">
					<CircleCheckIcon class="size-5 text-green-600 dark:text-green-500" />
					You're all set
				</Card.Title>
				<Card.Description>
					RD Client is connected to your provider. To use it from Sonarr or Radarr, add it as a
					qBittorrent download client; see the
					<ExternalLink href="https://github.com/pahorema/rd-client-rs#readme">
						documentation
					</ExternalLink>
					for details.
				</Card.Description>
			</Card.Header>
			<Card.Footer>
				<Button class="w-full" onclick={() => goto(resolve(''))}>Go to torrents</Button>
			</Card.Footer>
		{/if}
	</Card.Root>
</AuthLayout>

{#snippet failure()}
	{#if error}
		<Alert.Root variant="destructive">
			<CircleAlertIcon />
			<Alert.Description>{error}</Alert.Description>
		</Alert.Root>
	{/if}
{/snippet}

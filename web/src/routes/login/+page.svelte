<script lang="ts">
	import { auth, errorMessage } from '#lib/api/index.js';
	import AuthLayout from '#lib/components/auth-layout.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Card from '#lib/components/ui/card/index.js';
	import * as Field from '#lib/components/ui/field/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import { onMount } from 'svelte';

	let userName = $state('');
	let password = $state('');
	let loggingIn = $state(false);
	let error = $state<string | null>(null);

	onMount(() => {
		// Skip the form when already signed in or when authentication is disabled.
		auth
			.isLoggedIn()
			.then(() => goto(resolve('')))
			.catch(() => undefined);
	});

	async function login(event: SubmitEvent) {
		event.preventDefault();
		loggingIn = true;
		error = null;

		try {
			await auth.login(userName, password);
			await goto(resolve(''));
		} catch (e) {
			error = errorMessage(e);
			loggingIn = false;
		}
	}
</script>

<svelte:head>
	<title>Log in · RD Client</title>
</svelte:head>

<AuthLayout>
	<Card.Root>
		<Card.Header>
			<Card.Title>Log in</Card.Title>
			<Card.Description>Enter your username and password to continue.</Card.Description>
		</Card.Header>
		<Card.Content>
			<form onsubmit={login}>
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
							autocomplete="current-password"
							required
							bind:value={password}
						/>
					</Field.Field>
					{#if error}
						<Alert.Root variant="destructive">
							<CircleAlertIcon />
							<Alert.Description>{error}</Alert.Description>
						</Alert.Root>
					{/if}
					<Button type="submit" disabled={loggingIn}>
						{#if loggingIn}<Spinner />{/if}
						Log in
					</Button>
				</Field.Group>
			</form>
		</Card.Content>
	</Card.Root>
</AuthLayout>

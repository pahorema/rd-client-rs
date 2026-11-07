<script lang="ts">
	import { errorMessage, settings as settingsApi } from '#lib/api/index.js';
	import type { Setting } from '#lib/api/types.js';
	import SettingField from '#lib/components/setting-field.svelte';
	import * as Alert from '#lib/components/ui/alert/index.js';
	import { Button } from '#lib/components/ui/button/index.js';
	import * as Card from '#lib/components/ui/card/index.js';
	import * as Field from '#lib/components/ui/field/index.js';
	import { Spinner } from '#lib/components/ui/spinner/index.js';
	import * as Tabs from '#lib/components/ui/tabs/index.js';
	import { formatSpeed } from '#lib/format.js';
	import { settingsTabs, settingUpdates } from '#lib/settings-form.js';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
	import { onMount } from 'svelte';

	type Outcome = { ok: true; text: string } | { ok: false; text: string } | null;

	let catalogue = $state<Setting[] | null>(null);
	let loadError = $state<string | null>(null);
	let active = $state('');
	let saving = $state(false);
	let saveOutcome = $state<Outcome>(null);
	let running = $state<string | null>(null);
	const outcomes = $state<Record<string, Outcome>>({});

	const tabs = $derived(catalogue ? settingsTabs(catalogue) : []);
	const canRegisterMagnetHandler =
		typeof window !== 'undefined' &&
		window.isSecureContext &&
		'registerProtocolHandler' in navigator;

	onMount(() => {
		settingsApi
			.list()
			.then((result) => {
				catalogue = result ?? [];
				active = settingsTabs(catalogue)[0]?.tab.key ?? 'Tests';
			})
			.catch((e: unknown) => (loadError = errorMessage(e)));
	});

	const valueOf = (key: string) => catalogue?.find((s) => s.key === key)?.value;

	async function save() {
		if (!catalogue) return;
		saving = true;
		saveOutcome = null;

		try {
			await settingsApi.update(settingUpdates(catalogue));
			saveOutcome = { ok: true, text: 'Settings saved.' };
		} catch (e) {
			saveOutcome = { ok: false, text: errorMessage(e) };
		} finally {
			saving = false;
		}
	}

	async function run(name: string, action: () => Promise<string>) {
		running = name;
		outcomes[name] = null;

		try {
			outcomes[name] = { ok: true, text: await action() };
		} catch (e) {
			outcomes[name] = { ok: false, text: errorMessage(e) };
		} finally {
			running = null;
		}
	}

	const tests = [
		{
			name: 'path',
			label: 'Test download path permissions',
			help: 'Checks that the download path exists and can be written to.',
			action: async () => {
				await settingsApi.testPath(String(valueOf('DownloadClient:DownloadPath') ?? ''));
				return 'The download path looks good.';
			}
		},
		{
			name: 'download',
			label: 'Test download speed',
			help: 'Downloads a test file from your provider (or a public speed-test server when it has none) and stops after 50 MB.',
			action: async () => `Download speed: ${formatSpeed(await settingsApi.testDownloadSpeed())}`
		},
		{
			name: 'write',
			label: 'Test write speed',
			help: 'Writes a temporary file to the download path to measure the disk speed.',
			action: async () => `Write speed: ${formatSpeed(await settingsApi.testWriteSpeed())}`
		}
	];

	function testAria2() {
		void run('aria2', async () => {
			const result = await settingsApi.testAria2cConnection(
				String(valueOf('DownloadClient:Aria2cUrl') ?? ''),
				String(valueOf('DownloadClient:Aria2cSecret') ?? '')
			);
			return `Connected to aria2 version ${result?.version ?? 'unknown'}.`;
		});
	}

	function registerMagnetHandler() {
		try {
			navigator.registerProtocolHandler(
				'magnet',
				`${location.origin}${location.pathname}add?magnet=%s`
			);
			outcomes.magnet = {
				ok: true,
				text: 'Your browser will ask you to confirm RD Client as the handler for magnet links.'
			};
		} catch (e) {
			outcomes.magnet = { ok: false, text: `Registration failed: ${errorMessage(e)}` };
		}
	}
</script>

<svelte:head>
	<title>Settings · RD Client</title>
</svelte:head>

<h1 class="text-xl font-semibold tracking-tight">Settings</h1>

{#if loadError}
	{@render outcome({ ok: false, text: loadError })}
{:else if !catalogue}
	<div class="flex justify-center py-16"><Spinner class="size-6 text-muted-foreground" /></div>
{:else}
	<Tabs.Root bind:value={active}>
		<Tabs.List class="h-auto flex-wrap">
			{#each tabs as { tab } (tab.key)}
				<Tabs.Trigger value={tab.key}>{tab.displayName ?? tab.key}</Tabs.Trigger>
			{/each}
			<Tabs.Trigger value="Tests">Diagnostics</Tabs.Trigger>
		</Tabs.List>

		{#each tabs as { tab, entries } (tab.key)}
			<Tabs.Content value={tab.key}>
				<Card.Root>
					{#if tab.description}
						<Card.Header><Card.Description>{tab.description}</Card.Description></Card.Header>
					{/if}
					<Card.Content>
						<Field.Group class="max-w-2xl gap-6">
							{#each entries as entry (entry.key)}
								{@const index = catalogue.indexOf(entry)}
								<SettingField bind:setting={catalogue[index]}>
									{#snippet extra()}
										{#if entry.key === 'DownloadClient:Aria2cSecret'}
											<div class="grid justify-items-start gap-2">
												<Button
													size="sm"
													variant="outline"
													disabled={running != null}
													onclick={testAria2}
												>
													{#if running === 'aria2'}<Spinner />{/if}
													Test aria2 connection
												</Button>
												{@render outcome(outcomes.aria2)}
											</div>
										{/if}
									{/snippet}
								</SettingField>
							{/each}
							{#if tab.key === 'General'}
								<Field.Field>
									<Field.Label>Magnet link handler</Field.Label>
									<div>
										<Button
											size="sm"
											variant="outline"
											disabled={!canRegisterMagnetHandler}
											onclick={registerMagnetHandler}
										>
											{canRegisterMagnetHandler
												? 'Register handler'
												: 'Unavailable in this browser'}
										</Button>
									</div>
									<Field.Description>
										{#if canRegisterMagnetHandler}
											Open magnet links from your browser directly in the add torrent page.
										{:else}
											Your browser does not support it, or RD Client is not served over HTTPS or
											localhost.
										{/if}
									</Field.Description>
									{@render outcome(outcomes.magnet)}
								</Field.Field>
							{/if}
						</Field.Group>
					</Card.Content>
					<Card.Footer class="flex-wrap gap-3">
						<Button onclick={save} disabled={saving}>
							{#if saving}<Spinner />{/if}
							Save settings
						</Button>
						{@render outcome(saveOutcome)}
					</Card.Footer>
				</Card.Root>
			</Tabs.Content>
		{/each}

		<Tabs.Content value="Tests">
			<Card.Root>
				<Card.Content>
					<Field.Group class="max-w-2xl gap-6">
						{#each tests as test (test.name)}
							<Field.Field>
								<Field.Label>{test.label}</Field.Label>
								<div>
									<Button
										size="sm"
										variant="outline"
										disabled={running != null}
										onclick={() => run(test.name, test.action)}
									>
										{#if running === test.name}<Spinner />{/if}
										Run test
									</Button>
								</div>
								<Field.Description>{test.help}</Field.Description>
								{@render outcome(outcomes[test.name])}
							</Field.Field>
						{/each}
					</Field.Group>
				</Card.Content>
			</Card.Root>
		</Tabs.Content>
	</Tabs.Root>
{/if}

{#snippet outcome(result: Outcome | undefined)}
	{#if result}
		<Alert.Root
			variant={result.ok ? 'default' : 'destructive'}
			class={result.ok ? 'text-green-700 dark:text-green-500' : ''}
		>
			{#if result.ok}<CircleCheckIcon />{:else}<CircleAlertIcon />{/if}
			<Alert.Description class={result.ok ? 'text-current' : ''}>{result.text}</Alert.Description>
		</Alert.Root>
	{/if}
{/snippet}

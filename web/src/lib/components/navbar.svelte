<script lang="ts">
	import logo from '#lib/assets/logo.svg';
	import { auth, errorMessage, settings } from '#lib/api/index.js';
	import ThemeToggle from '#lib/components/theme-toggle.svelte';
	import { buttonVariants } from '#lib/components/ui/button/index.js';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.js';
	import { cn } from '#lib/utils.js';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import CircleUserIcon from '@lucide/svelte/icons/circle-user';
	import ListIcon from '@lucide/svelte/icons/list';
	import MenuIcon from '@lucide/svelte/icons/menu';
	import { onMount } from 'svelte';

	/** Primary navigation; `prefixes` are the paths that mark the link as current. */
	const links = [
		{ href: resolve(''), prefixes: ['/torrent/'], exact: '/', label: 'Torrents', icon: ListIcon }
	];

	let version = $state<string | null>(null);
	let logoutError = $state<string | null>(null);

	function isActive(link: (typeof links)[number]) {
		const path = page.url.pathname;

		return path === link.exact || link.prefixes.some((prefix) => path.startsWith(prefix));
	}

	async function logout() {
		logoutError = null;

		try {
			await auth.logout();
			await goto(resolve('login'));
		} catch (error) {
			logoutError = errorMessage(error);
		}
	}

	onMount(() => {
		settings
			.version()
			.then((result) => (version = result?.version ?? null))
			.catch(() => (version = null));
	});
</script>

<header class="sticky top-0 z-40 border-b bg-background/80 backdrop-blur">
	<nav class="mx-auto flex h-14 max-w-7xl items-center gap-2 px-4">
		<a href={resolve('')} class="mr-2 flex items-center gap-2 font-semibold">
			<img src={logo} alt="" class="size-7" />
			<span class="hidden sm:inline">RD Client</span>
		</a>

		<div class="hidden items-center gap-1 md:flex">
			{#each links as link (link.href)}
				<a
					href={link.href}
					class={cn(
						buttonVariants({ variant: 'ghost', size: 'sm' }),
						isActive(link) && 'bg-accent text-accent-foreground'
					)}
					aria-current={isActive(link) ? 'page' : undefined}
				>
					<link.icon />
					{link.label}
				</a>
			{/each}
		</div>

		<div class="ml-auto flex items-center gap-1">
			<ThemeToggle />

			<DropdownMenu.Root>
				<DropdownMenu.Trigger
					class={buttonVariants({ variant: 'ghost', size: 'icon' })}
					aria-label="Account"
					title="Account"
				>
					<CircleUserIcon />
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end" class="min-w-44">
					<DropdownMenu.Item onSelect={logout}>Log out</DropdownMenu.Item>
					<DropdownMenu.Separator />
					<DropdownMenu.Item>
						{#snippet child({ props })}
							<a
								{...props}
								href="https://github.com/pahorema/rd-client-rs"
								target="_blank"
								rel="noopener noreferrer"
							>
								Version {version ?? '…'}
							</a>
						{/snippet}
					</DropdownMenu.Item>
				</DropdownMenu.Content>
			</DropdownMenu.Root>

			<DropdownMenu.Root>
				<DropdownMenu.Trigger
					class={cn(buttonVariants({ variant: 'ghost', size: 'icon' }), 'md:hidden')}
					aria-label="Menu"
				>
					<MenuIcon />
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end" class="min-w-44">
					{#each links as link (link.href)}
						<DropdownMenu.Item onSelect={() => goto(link.href)}>
							<link.icon />
							{link.label}
						</DropdownMenu.Item>
					{/each}
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		</div>
	</nav>
	{#if logoutError}
		<p class="mx-auto max-w-7xl px-4 pb-2 text-sm text-destructive">
			Could not log out: {logoutError}
		</p>
	{/if}
</header>

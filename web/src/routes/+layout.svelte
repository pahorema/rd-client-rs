<script lang="ts">
	import './layout.css';
	import favicon from '#lib/assets/logo.svg';
	import { onAuthRequired } from '#lib/api/index.js';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { ModeWatcher } from 'mode-watcher';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	onAuthRequired((requirement) => {
		const target = requirement === 'setup' ? 'setup' : 'login';

		if (page.route.id !== `/${target}`) {
			void goto(resolve(target));
		}
	});
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
</svelte:head>

<ModeWatcher />

{@render children()}

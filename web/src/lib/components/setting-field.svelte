<script lang="ts">
	import type { Setting } from '#lib/api/types.js';
	import OptionSelect from '#lib/components/option-select.svelte';
	import { Checkbox } from '#lib/components/ui/checkbox/index.js';
	import * as Field from '#lib/components/ui/field/index.js';
	import { Input } from '#lib/components/ui/input/index.js';
	import { descriptionHtml, enumOptions } from '#lib/settings-form.js';
	import type { Snippet } from 'svelte';

	let {
		setting = $bindable(),
		error,
		extra
	}: { setting: Setting; error?: string; extra?: Snippet } = $props();

	const id = $derived(`setting-${setting.key.replaceAll(':', '-')}`);
</script>

{#if setting.type === 'Object'}
	<h3 class="pt-4 text-base font-semibold first:pt-0">{setting.displayName}</h3>
{:else}
	<Field.Field
		orientation={setting.type === 'Boolean' ? 'horizontal' : 'vertical'}
		data-invalid={error ? true : undefined}
	>
		{#if setting.type === 'Boolean'}
			<Checkbox
				{id}
				checked={setting.value === true}
				onCheckedChange={(checked) => (setting.value = checked)}
			/>
			<Field.Content>
				<Field.Label for={id}>{setting.displayName}</Field.Label>
				{@render description()}
			</Field.Content>
		{:else}
			<Field.Label for={id}>{setting.displayName}</Field.Label>
			{#if setting.type === 'Enum'}
				<OptionSelect
					{id}
					options={enumOptions(setting)}
					value={setting.value == null ? null : Number(setting.value)}
					onValueChange={(value) => (setting.value = value)}
				/>
			{:else if setting.type === 'Int32'}
				<Input
					{id}
					type="number"
					step="1"
					aria-invalid={error ? true : undefined}
					value={setting.value ?? ''}
					oninput={(event) => {
						const raw = event.currentTarget.value;
						setting.value = raw === '' ? null : Number(raw);
					}}
				/>
			{:else}
				<Input
					{id}
					value={setting.value == null ? '' : String(setting.value)}
					oninput={(event) => (setting.value = event.currentTarget.value)}
				/>
			{/if}
			{@render description()}
			{#if error}<Field.Error>{error}</Field.Error>{/if}
		{/if}
		{@render extra?.()}
	</Field.Field>
{/if}

{#snippet description()}
	{#if setting.description}
		<Field.Description class="[&_a]:text-primary [&_a]:underline-offset-4 [&_a:hover]:underline">
			<!-- Descriptions come from the server's settings catalogue and contain links. -->
			<!-- eslint-disable-next-line svelte/no-at-html-tags -->
			{@html descriptionHtml(setting.description)}
		</Field.Description>
	{/if}
{/snippet}

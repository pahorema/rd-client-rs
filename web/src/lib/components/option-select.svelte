<script lang="ts" module>
	export interface Option<V extends number> {
		value: V;
		label: string;
	}
</script>

<script lang="ts" generics="T extends number">
	import * as Select from '#lib/components/ui/select/index.js';
	import { cn } from '#lib/utils.js';

	let {
		options,
		value = $bindable(),
		id,
		placeholder = 'Select…',
		disabled = false,
		class: className,
		onValueChange
	}: {
		options: readonly Option<T>[];
		/** `null` shows the placeholder (e.g. values that differ between torrents). */
		value: T | null;
		id?: string;
		placeholder?: string;
		disabled?: boolean;
		class?: string;
		onValueChange?: (value: T) => void;
	} = $props();

	const selected = $derived(options.find((option) => option.value === value));

	function change(raw: string) {
		const option = options.find((candidate) => String(candidate.value) === raw);

		if (option) {
			value = option.value;
			onValueChange?.(option.value);
		}
	}
</script>

<Select.Root
	type="single"
	value={value == null ? '' : String(value)}
	onValueChange={change}
	{disabled}
>
	<Select.Trigger {id} class={cn('w-full', className)}>
		<span class={cn(!selected && 'text-muted-foreground')}>{selected?.label ?? placeholder}</span>
	</Select.Trigger>
	<Select.Content>
		{#each options as option (option.value)}
			<Select.Item value={String(option.value)} label={option.label}>{option.label}</Select.Item>
		{/each}
	</Select.Content>
</Select.Root>

import type { Setting, SettingUpdate } from './api/types.js';

/** A top-level settings group and the entries below it (including nested headings). */
export interface SettingsTab {
	tab: Setting;
	entries: Setting[];
}

/** Tabs are the keys without `:`; each holds the entries whose key starts with `<tab>:`. */
export function settingsTabs(settings: readonly Setting[]): SettingsTab[] {
	return settings
		.filter((setting) => !setting.key.includes(':'))
		.map((tab) => ({
			tab,
			entries: settings.filter((setting) => setting.key.startsWith(`${tab.key}:`))
		}));
}

/** Every value entry of the catalogue, as sent back on save. */
export function settingUpdates(settings: readonly Setting[]): SettingUpdate[] {
	return settings
		.filter((setting) => setting.type !== 'Object')
		.map((setting) => ({
			key: setting.key,
			value:
				setting.type === 'Enum' && setting.value != null ? Number(setting.value) : setting.value
		}));
}

/** Option list of an `Enum` setting, ordered by numeric value. */
export function enumOptions(setting: Setting): { value: number; label: string }[] {
	return Object.entries(setting.enumValues ?? {})
		.map(([value, label]) => ({ value: Number(value), label }))
		.sort((a, b) => a.value - b.value);
}

/** Descriptions are trusted HTML from the server; newlines become line breaks. */
export function descriptionHtml(description: string | null): string {
	return (description ?? '').replace(/\r\n|\r|\n/g, '<br />');
}

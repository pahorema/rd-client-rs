import { describe, expect, it } from 'vitest';
import type { Setting } from './api/types.js';
import { descriptionHtml, enumOptions, settingsTabs, settingUpdates } from './settings-form.js';

function entry(key: string, type: Setting['type'], value: Setting['value'] = null): Setting {
	return { key, value, displayName: key, description: null, type, enumValues: null };
}

const catalogue: Setting[] = [
	entry('General', 'Object'),
	entry('General:LogLevel', 'Enum', 4),
	entry('General:Categories', 'String', 'tv'),
	entry('Gui', 'Object'),
	entry('Gui:Default', 'Object'),
	entry('Gui:Default:Priority', 'Int32', 0),
	entry('Gui:Default:OnlyDownloadAvailableFiles', 'Boolean', true)
];

describe('settingsTabs', () => {
	it('groups entries under the keys without a colon', () => {
		const tabs = settingsTabs(catalogue);

		expect(tabs.map((t) => t.tab.key)).toEqual(['General', 'Gui']);
		expect(tabs[1].entries.map((e) => e.key)).toEqual([
			'Gui:Default',
			'Gui:Default:Priority',
			'Gui:Default:OnlyDownloadAvailableFiles'
		]);
	});
});

describe('settingUpdates', () => {
	it('sends every value entry with enums as numbers', () => {
		const edited = catalogue.map((s) => (s.key === 'General:LogLevel' ? { ...s, value: '2' } : s));

		expect(settingUpdates(edited)).toEqual([
			{ key: 'General:LogLevel', value: 2 },
			{ key: 'General:Categories', value: 'tv' },
			{ key: 'Gui:Default:Priority', value: 0 },
			{ key: 'Gui:Default:OnlyDownloadAvailableFiles', value: true }
		]);
	});
});

describe('enumOptions', () => {
	it('orders options numerically', () => {
		const setting = { ...entry('A', 'Enum'), enumValues: { '10': 'Ten', '2': 'Two' } };

		expect(enumOptions(setting)).toEqual([
			{ value: 2, label: 'Two' },
			{ value: 10, label: 'Ten' }
		]);
	});
});

describe('descriptionHtml', () => {
	it('turns newlines into line breaks and keeps markup', () => {
		expect(descriptionHtml('See\n<a href="x">docs</a>\r\nnow')).toBe(
			'See<br /><a href="x">docs</a><br />now'
		);
		expect(descriptionHtml(null)).toBe('');
	});
});

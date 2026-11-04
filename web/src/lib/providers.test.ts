import { describe, expect, it } from 'vitest';
import { Provider } from './api/types.js';
import { providerByName, providers } from './providers.js';

describe('providers', () => {
	it('lists every provider once in numeric order', () => {
		expect(providers.map((p) => p.value)).toEqual(Object.values(Provider));
	});

	it('links to the provider sites without referral parameters', () => {
		for (const provider of providers) {
			expect(new URL(provider.website).search).toBe('');
		}
	});

	it('finds providers by profile name regardless of case', () => {
		expect(providerByName('RealDebrid')?.value).toBe(Provider.RealDebrid);
		expect(providerByName('HIGHWAY')?.value).toBe(Provider.Highway);
		expect(providerByName('Highway')?.value).toBe(Provider.Highway);
		expect(providerByName(null)).toBeUndefined();
		expect(providerByName('Unknown')).toBeUndefined();
	});
});

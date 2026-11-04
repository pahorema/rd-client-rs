import { Provider } from './api/types.js';

/** Display information for a debrid provider. */
export interface ProviderInfo {
	value: Provider;
	/** Name as reported by the profile endpoint. */
	name: string;
	label: string;
	website: string;
	/** Page where the user finds the API key, when the provider has one. */
	apiKeyUrl: string | null;
	/** Extra instructions when there is no API key page. */
	apiKeyHelp: string | null;
}

export const providers: readonly ProviderInfo[] = [
	{
		value: Provider.RealDebrid,
		name: 'RealDebrid',
		label: 'Real-Debrid',
		website: 'https://real-debrid.com/',
		apiKeyUrl: 'https://real-debrid.com/apitoken',
		apiKeyHelp: null
	},
	{
		value: Provider.AllDebrid,
		name: 'AllDebrid',
		label: 'AllDebrid',
		website: 'https://alldebrid.com/',
		apiKeyUrl: 'https://alldebrid.com/apikeys/',
		apiKeyHelp: null
	},
	{
		value: Provider.Premiumize,
		name: 'Premiumize',
		label: 'Premiumize',
		website: 'https://www.premiumize.me/',
		apiKeyUrl: 'https://www.premiumize.me/account',
		apiKeyHelp: null
	},
	{
		value: Provider.TorBox,
		name: 'TorBox',
		label: 'TorBox',
		website: 'https://torbox.app/',
		apiKeyUrl: 'https://torbox.app/settings',
		apiKeyHelp: null
	},
	{
		value: Provider.DebridLink,
		name: 'DebridLink',
		label: 'DebridLink',
		website: 'https://debrid-link.com/',
		apiKeyUrl: 'https://debrid-link.com/webapp/apikey',
		apiKeyHelp: null
	},
	{
		value: Provider.Highway,
		name: 'Highway',
		label: 'HIGHWAY',
		website: 'https://high-way.me/',
		apiKeyUrl: null,
		apiKeyHelp:
			'Log in to your HIGHWAY Center, open "Sitzungen" and use "API-Token erstellen" to create a token.'
	}
];

/** Looks up a provider by the name the profile endpoint reports (case-insensitive). */
export function providerByName(name: string | null | undefined): ProviderInfo | undefined {
	const key = name?.toLowerCase();

	return providers.find((provider) => provider.name.toLowerCase() === key);
}

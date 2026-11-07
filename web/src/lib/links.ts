/**
 * `href` when it is an absolute `http:` or `https:` URL, otherwise `null`. Links can come
 * from the debrid provider, so other schemes (`javascript:`, `data:`, …) are never linked.
 */
export function externalHref(href: string): string | null {
	try {
		const { protocol } = new URL(href);

		return protocol === 'http:' || protocol === 'https:' ? href : null;
	} catch {
		return null;
	}
}

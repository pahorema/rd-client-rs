import { describe, expect, it } from 'vitest';
import { externalHref } from './links.js';

describe('externalHref', () => {
	it('keeps web links', () => {
		expect(externalHref('https://example.com/d/1?x=%20')).toBe('https://example.com/d/1?x=%20');
		expect(externalHref('HTTP://10.0.0.2:8080/file')).toBe('HTTP://10.0.0.2:8080/file');
	});

	it('drops other schemes and relative or malformed links', () => {
		expect(externalHref('javascript:alert(1)')).toBeNull();
		expect(externalHref(' JavaScript:alert(1)')).toBeNull();
		expect(externalHref('data:text/html,<p>x</p>')).toBeNull();
		expect(externalHref('/Api/Torrents')).toBeNull();
		expect(externalHref('not a url')).toBeNull();
	});
});

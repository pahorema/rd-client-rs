import { describe, expect, it } from 'vitest';
import { compareVersions, isNewerVersion } from './version.js';

describe('compareVersions', () => {
	it('orders by number, not text', () => {
		expect(compareVersions('1.10.0', '1.9.0')).toBeGreaterThan(0);
		expect(compareVersions('v2.0.0', '2.0.0')).toBe(0);
		expect(compareVersions('1.0.0+build.5', '1.0.0')).toBe(0);
	});

	it('orders pre-releases before their release', () => {
		expect(compareVersions('1.0.0-beta.2', '1.0.0')).toBeLessThan(0);
		expect(compareVersions('1.0.0-beta.10', '1.0.0-beta.2')).toBeGreaterThan(0);
		expect(compareVersions('1.0.0-alpha', '1.0.0-alpha.1')).toBeLessThan(0);
		expect(compareVersions('1.0.0-1', '1.0.0-alpha')).toBeLessThan(0);
	});

	it('returns null for other version strings', () => {
		expect(compareVersions('nightly', '1.0.0')).toBeNull();
	});
});

describe('isNewerVersion', () => {
	it('is false for a build newer than the latest release', () => {
		expect(isNewerVersion('1.2.0', '1.3.0-dev')).toBe(false);
		expect(isNewerVersion('1.2.0', '1.2.0')).toBe(false);
	});

	it('is true for a newer release', () => {
		expect(isNewerVersion('1.10.0', '1.9.3')).toBe(true);
		expect(isNewerVersion('1.2.0', '1.2.0-rc.1')).toBe(true);
	});

	it('falls back to inequality for non-semantic versions', () => {
		expect(isNewerVersion('2024-06', 'nightly')).toBe(true);
		expect(isNewerVersion('nightly', 'nightly')).toBe(false);
	});
});

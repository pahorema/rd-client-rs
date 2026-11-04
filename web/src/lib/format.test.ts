import { describe, expect, it } from 'vitest';
import {
	decodeUrl,
	formatDate,
	formatDateTime,
	formatSize,
	formatSpeed,
	parseTimestamp
} from './format.js';

describe('formatSize', () => {
	it('formats missing values as zero bytes', () => {
		expect(formatSize(null)).toBe('0 B');
		expect(formatSize(undefined)).toBe('0 B');
		expect(formatSize(Number.NaN)).toBe('0 B');
	});

	it('keeps values below one kilobyte in bytes', () => {
		expect(formatSize(0)).toBe('0 B');
		expect(formatSize(1023)).toBe('1023 B');
	});

	it('uses binary multiples with at most two decimals', () => {
		expect(formatSize(1024)).toBe('1 KB');
		expect(formatSize(1536)).toBe('1.5 KB');
		expect(formatSize(1024 * 1024 * 1.234567)).toBe('1.23 MB');
		expect(formatSize(5 * 1024 ** 3)).toBe('5 GB');
		expect(formatSize(2 * 1024 ** 4)).toBe('2 TB');
	});

	it('stops at exabytes', () => {
		expect(formatSize(2048 * 1024 ** 6)).toBe('2048 EB');
	});
});

describe('formatSpeed', () => {
	it('appends a per-second suffix', () => {
		expect(formatSpeed(1024 * 1024 * 3)).toBe('3 MB/s');
		expect(formatSpeed(null)).toBe('0 B/s');
	});
});

describe('parseTimestamp', () => {
	it('accepts fractions longer than milliseconds', () => {
		expect(parseTimestamp('2025-01-31T10:20:30.1234567+00:00')?.toISOString()).toBe(
			'2025-01-31T10:20:30.123Z'
		);
	});

	it('honours the offset', () => {
		expect(parseTimestamp('2025-01-31T12:00:00+02:00')?.toISOString()).toBe(
			'2025-01-31T10:00:00.000Z'
		);
	});

	it('returns null for missing or invalid input', () => {
		expect(parseTimestamp(null)).toBeNull();
		expect(parseTimestamp('garbage')).toBeNull();
	});
});

describe('formatDateTime', () => {
	it('returns an empty string for missing or invalid timestamps', () => {
		expect(formatDateTime(null)).toBe('');
		expect(formatDateTime('not a date')).toBe('');
		expect(formatDate(undefined)).toBe('');
	});

	it('formats valid timestamps', () => {
		expect(formatDateTime('2025-01-31T10:20:30.1234567+00:00')).toContain('2025');
		expect(formatDate('2025-01-31T10:20:30+00:00')).toContain('2025');
	});
});

describe('decodeUrl', () => {
	it('decodes percent-encoded characters', () => {
		expect(decodeUrl('https://host/My%20File.mkv')).toBe('https://host/My File.mkv');
	});

	it('returns malformed input unchanged', () => {
		expect(decodeUrl('https://host/%E0%A4%A')).toBe('https://host/%E0%A4%A');
	});
});

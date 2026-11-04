import { describe, expect, it } from 'vitest';
import { compareSortValues, getSortFieldValue, sortItems } from './sort.js';

describe('getSortFieldValue', () => {
	it('reads nested properties', () => {
		expect(getSortFieldValue({ a: { b: { c: 3 } } }, 'a.b.c')).toBe(3);
		expect(getSortFieldValue({ files: [1, 2] }, 'files.length')).toBe(2);
	});

	it('returns undefined for missing paths', () => {
		expect(getSortFieldValue({ a: null }, 'a.b')).toBeUndefined();
		expect(getSortFieldValue(null, 'a')).toBeUndefined();
		expect(getSortFieldValue({ a: 1 }, '')).toBeUndefined();
	});
});

describe('compareSortValues', () => {
	it('compares strings case-insensitively and numerically', () => {
		expect(compareSortValues('abc', 'ABC')).toBe(0);
		expect(compareSortValues('file 2', 'file 10')).toBeLessThan(0);
	});

	it('compares numbers, booleans and dates', () => {
		expect(compareSortValues(1, 2)).toBeLessThan(0);
		expect(compareSortValues(true, false)).toBeGreaterThan(0);
		expect(compareSortValues(new Date(2), new Date(1))).toBeGreaterThan(0);
	});

	it('orders missing values last', () => {
		expect(compareSortValues(null, 1)).toBe(1);
		expect(compareSortValues(1, undefined)).toBe(-1);
	});
});

describe('sortItems', () => {
	const items = [
		{ name: 'b', size: 2 },
		{ name: 'a', size: null },
		{ name: 'c', size: 1 }
	];

	it('sorts ascending and descending without mutating the input', () => {
		expect(sortItems(items, 'name').map((m) => m.name)).toEqual(['a', 'b', 'c']);
		expect(sortItems(items, 'name', 'desc').map((m) => m.name)).toEqual(['c', 'b', 'a']);
		expect(items.map((m) => m.name)).toEqual(['b', 'a', 'c']);
	});

	it('keeps missing values last in both directions', () => {
		expect(sortItems(items, 'size').map((m) => m.size)).toEqual([1, 2, null]);
		expect(sortItems(items, 'size', 'desc').map((m) => m.size)).toEqual([2, 1, null]);
	});

	it('uses a custom accessor', () => {
		const sorted = sortItems(items, 'length', 'desc', (item) => item.name.charCodeAt(0));

		expect(sorted.map((m) => m.name)).toEqual(['c', 'b', 'a']);
	});
});

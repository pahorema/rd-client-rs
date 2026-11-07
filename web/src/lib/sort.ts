export type SortDirection = 'asc' | 'desc';

/** Reads a dotted property path (`a.b.c`) from a value, `undefined` when absent. */
export function getSortFieldValue(item: unknown, field: string): unknown {
	if (item == null || !field) {
		return undefined;
	}

	return field.split('.').reduce<unknown>((value, key) => {
		if (value == null || typeof value !== 'object') {
			return undefined;
		}

		return (value as Record<string, unknown>)[key];
	}, item);
}

const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });

/**
 * Compares two values for sorting: strings case-insensitively, dates and
 * booleans by value, and missing values after everything else.
 */
export function compareSortValues(left: unknown, right: unknown): number {
	if (left === right) {
		return 0;
	}

	if (left == null) {
		return 1;
	}

	if (right == null) {
		return -1;
	}

	if (typeof left === 'string' && typeof right === 'string') {
		return collator.compare(left, right);
	}

	if (left instanceof Date && right instanceof Date) {
		return left.getTime() - right.getTime();
	}

	if (typeof left === 'boolean' && typeof right === 'boolean') {
		return Number(left) - Number(right);
	}

	const a = left as string | number | bigint;
	const b = right as string | number | bigint;

	if (a < b) {
		return -1;
	}

	if (a > b) {
		return 1;
	}

	return 0;
}

/**
 * Returns a sorted copy of `items` ordered by `field`. Missing values sort
 * last in both directions. `accessor` runs once per item, not once per comparison.
 */
export function sortItems<T>(
	items: readonly T[],
	field: string,
	direction: SortDirection = 'asc',
	accessor: (item: T, field: string) => unknown = getSortFieldValue
): T[] {
	const sign = direction === 'asc' ? 1 : -1;
	const keyed = items.map((item) => ({ item, key: accessor(item, field) }));

	keyed.sort(({ key: a }, { key: b }) => {
		if (a == null || b == null) {
			return compareSortValues(a, b);
		}

		return compareSortValues(a, b) * sign;
	});

	return keyed.map(({ item }) => item);
}

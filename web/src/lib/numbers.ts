export const INT32_MIN = -2_147_483_648;
const INT32_MAX = 2_147_483_647;

/**
 * Why `value` from a number input cannot be sent as a server `i32`, or `null` when it can.
 * Svelte binds an empty number input as `null`, which is only accepted when `optional`.
 */
export function wholeNumberError(
	value: number | null | undefined,
	{ min = 0, optional = false }: { min?: number; optional?: boolean } = {}
): string | null {
	if (value == null) {
		return optional ? null : 'Enter a number.';
	}

	const lowest = Math.max(min, INT32_MIN);

	if (!Number.isInteger(value) || value < lowest || value > INT32_MAX) {
		return `Enter a whole number from ${lowest} to ${INT32_MAX}.`;
	}

	return null;
}

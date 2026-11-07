const SEMVER = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/;

function compareIdentifiers(a: string, b: string): number {
	const numericA = /^\d+$/.test(a);
	const numericB = /^\d+$/.test(b);

	if (numericA && numericB) return Number(a) - Number(b);
	if (numericA !== numericB) return numericA ? -1 : 1;

	return a < b ? -1 : a > b ? 1 : 0;
}

/**
 * Orders two semantic versions (an optional `v` prefix is allowed; build metadata is ignored):
 * negative when `a` is older, positive when newer, 0 when equal, `null` when either is not
 * a semantic version.
 */
export function compareVersions(a: string, b: string): number | null {
	const left = SEMVER.exec(a.trim());
	const right = SEMVER.exec(b.trim());

	if (!left || !right) return null;

	for (let i = 1; i <= 3; i += 1) {
		const difference = Number(left[i]) - Number(right[i]);
		if (difference !== 0) return difference;
	}

	// A pre-release sorts before its release.
	const preLeft = left[4]?.split('.') ?? [];
	const preRight = right[4]?.split('.') ?? [];
	if (preLeft.length === 0 || preRight.length === 0) return preRight.length - preLeft.length;

	for (let i = 0; i < Math.min(preLeft.length, preRight.length); i += 1) {
		const difference = compareIdentifiers(preLeft[i], preRight[i]);
		if (difference !== 0) return difference;
	}

	return preLeft.length - preRight.length;
}

/**
 * Whether `latest` is a newer release than `current`. Versions that are not semantic
 * versions are only compared for equality.
 */
export function isNewerVersion(latest: string, current: string): boolean {
	const order = compareVersions(latest, current);

	return order == null ? latest.trim() !== current.trim() : order > 0;
}

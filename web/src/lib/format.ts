import type { Timestamp } from './api/types.js';

const SIZE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB'] as const;

/**
 * Formats a byte count with binary multiples and at most two decimals,
 * e.g. `1536` ⇒ `1.5 KB`. Missing values format as `0 B`.
 */
export function formatSize(bytes: number | null | undefined): string {
	if (bytes == null || !Number.isFinite(bytes)) {
		return '0 B';
	}

	let size = bytes;
	let unit = 0;

	while (size >= 1024 && unit < SIZE_UNITS.length - 1) {
		size /= 1024;
		unit += 1;
	}

	return `${Number(size.toFixed(2))} ${SIZE_UNITS[unit]}`;
}

/** Formats a transfer rate in bytes per second, e.g. `1.5 MB/s`. */
export function formatSpeed(bytesPerSecond: number | null | undefined): string {
	return `${formatSize(bytesPerSecond)}/s`;
}

const numberFormats = new Map<number, Intl.NumberFormat>();
const dateTimeFormat = new Intl.DateTimeFormat(undefined, {
	dateStyle: 'medium',
	timeStyle: 'medium'
});
const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });

/** Formats a number with the user's locale grouping. */
export function formatNumber(value: number | null | undefined, maximumFractionDigits = 0): string {
	let format = numberFormats.get(maximumFractionDigits);

	if (!format) {
		format = new Intl.NumberFormat(undefined, { maximumFractionDigits });
		numberFormats.set(maximumFractionDigits, format);
	}

	return format.format(value ?? 0);
}

/**
 * Parses an RFC 3339 timestamp. Fractions longer than milliseconds are
 * truncated first because not every engine accepts them.
 */
export function parseTimestamp(timestamp: Timestamp | null | undefined): Date | null {
	if (!timestamp) {
		return null;
	}

	const date = new Date(timestamp.replace(/(\.\d{3})\d+/, '$1'));

	return Number.isNaN(date.getTime()) ? null : date;
}

/** Formats a timestamp as a medium date and time in the user's locale. */
export function formatDateTime(timestamp: Timestamp | null | undefined): string {
	const date = parseTimestamp(timestamp);

	return date ? dateTimeFormat.format(date) : '';
}

/** Formats a timestamp as a date in the user's locale. */
export function formatDate(timestamp: Timestamp | null | undefined): string {
	const date = parseTimestamp(timestamp);

	return date ? dateFormat.format(date) : '';
}

/** Decodes a percent-encoded URL for display, returning it unchanged when malformed. */
export function decodeUrl(url: string): string {
	try {
		return decodeURI(url);
	} catch {
		return url;
	}
}

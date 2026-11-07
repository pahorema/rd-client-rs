import { describe, expect, it, vi } from 'vitest';
import type { AvailableFile } from './api/types.js';
import { FileCheck } from './file-check.svelte.js';

const filesA: AvailableFile[] = [{ filename: 'a.mkv', filesize: 1 }];
const filesB: AvailableFile[] = [{ filename: 'b.mkv', filesize: 2 }];

describe('FileCheck', () => {
	it('keeps the reply for the current source when an earlier one arrives last', async () => {
		const loaded = vi.fn();
		const check = new FileCheck(loaded);
		const a = Promise.withResolvers<AvailableFile[]>();
		const b = Promise.withResolvers<AvailableFile[]>();

		const first = check.check('magnet:a', () => a.promise);
		const second = check.check('magnet:b', () => b.promise);
		b.resolve(filesB);
		await second;
		a.resolve(filesA);
		await first;

		expect(check.available).toEqual(filesB);
		expect(check.checking).toBe(false);
		expect(loaded).toHaveBeenCalledTimes(1);
		expect(loaded).toHaveBeenCalledWith(filesB);
	});

	it('ignores a late failure of a replaced source and stays checking for the new one', async () => {
		const check = new FileCheck(() => undefined);
		const a = Promise.withResolvers<AvailableFile[]>();
		const b = Promise.withResolvers<AvailableFile[]>();

		const first = check.check('magnet:a', () => a.promise);
		void check.check('magnet:b', () => b.promise);
		a.reject(new Error('timed out'));
		await first;

		expect(check.error).toBeNull();
		expect(check.checking).toBe(true);
	});

	it('checks a source once and retries it after a failure', async () => {
		const check = new FileCheck(() => undefined);
		const load = vi.fn().mockRejectedValueOnce(new Error('offline')).mockResolvedValue(filesA);

		await check.check('magnet:a', load);
		expect(check.error).toBe('offline');

		await check.check('magnet:a', load);
		await check.check('magnet:a', load);

		expect(load).toHaveBeenCalledTimes(2);
		expect(check.available).toEqual(filesA);
		expect(check.error).toBeNull();
	});

	it('clears the files when there is nothing to check', async () => {
		const check = new FileCheck(() => undefined);
		await check.check('magnet:a', () => Promise.resolve(filesA));
		await check.check('', () => Promise.resolve(filesB));

		expect(check.available).toBeNull();
		expect(check.checking).toBe(false);
	});
});

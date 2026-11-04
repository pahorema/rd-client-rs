import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, errorMessage, onAuthRequired, request } from './client.js';

const fetchMock = vi.fn<typeof fetch>();

beforeEach(() => {
	vi.stubGlobal('fetch', fetchMock);
});

afterEach(() => {
	fetchMock.mockReset();
	vi.unstubAllGlobals();
	onAuthRequired(null);
});

describe('request', () => {
	it('sends relative paths with same-origin credentials', async () => {
		fetchMock.mockResolvedValue(new Response('[1,2]', { status: 200 }));

		await expect(request<number[]>('Api/Torrents')).resolves.toEqual([1, 2]);

		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('Api/Torrents');
		expect(init?.method).toBe('GET');
		expect(init?.credentials).toBe('same-origin');
	});

	it('serializes JSON bodies', async () => {
		fetchMock.mockResolvedValue(new Response(null, { status: 200 }));

		await request('Api/Settings', { method: 'PUT', json: [{ key: 'a', value: 1 }] });

		const init = fetchMock.mock.calls[0][1];
		expect(init?.body).toBe('[{"key":"a","value":1}]');
		expect((init?.headers as Record<string, string>)['Content-Type']).toBe('application/json');
	});

	it('sends multipart bodies without overriding the content type', async () => {
		fetchMock.mockResolvedValue(new Response(null, { status: 200 }));
		const form = new FormData();

		await request('Api/Torrents/CheckFiles', { method: 'POST', form });

		const init = fetchMock.mock.calls[0][1];
		expect(init?.body).toBe(form);
		expect((init?.headers as Record<string, string>)['Content-Type']).toBeUndefined();
	});

	it('resolves empty and no-content responses with null', async () => {
		fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
		fetchMock.mockResolvedValueOnce(new Response('', { status: 200 }));

		await expect(request('Api/Torrents/DiskSpaceStatus')).resolves.toBeNull();
		await expect(request('Api/Authentication/Logout')).resolves.toBeNull();
	});

	it('surfaces plain-text error bodies', async () => {
		fetchMock.mockResolvedValue(new Response('Invalid credentials', { status: 400 }));

		const error = await request('Api/Authentication/Login').catch((e: unknown) => e);

		expect(error).toBeInstanceOf(ApiError);
		expect((error as ApiError).status).toBe(400);
		expect(errorMessage(error)).toBe('Invalid credentials');
	});

	it('describes errors without a body by their status', async () => {
		fetchMock.mockResolvedValue(new Response(null, { status: 500, statusText: 'Server Error' }));

		await expect(request('Api/Torrents')).rejects.toThrow(
			'Request failed with status 500 Server Error'
		);
	});

	it('reports non-JSON success bodies', async () => {
		fetchMock.mockResolvedValue(new Response('<html>', { status: 200 }));

		await expect(request('Api/Torrents')).rejects.toThrow('Unexpected response from the server');
	});

	it('asks for setup on 402 and for login on 401 and 403', async () => {
		const handler = vi.fn();
		onAuthRequired(handler);

		for (const status of [402, 401, 403, 400]) {
			fetchMock.mockResolvedValueOnce(new Response(null, { status }));
			await request('Api/Torrents').catch(() => undefined);
		}

		expect(handler.mock.calls).toEqual([['setup'], ['login'], ['login']]);
	});
});

describe('errorMessage', () => {
	it('stringifies non-error values', () => {
		expect(errorMessage('plain')).toBe('plain');
		expect(errorMessage(new Error('boom'))).toBe('boom');
	});
});

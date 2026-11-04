import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { auth, settings, torrents } from './index.js';
import {
	DownloadClient,
	Provider,
	TorrentDownloadAction,
	TorrentFinishedAction,
	TorrentHostDownloadAction,
	type NewTorrent,
	type Torrent
} from './types.js';

const fetchMock = vi.fn<typeof fetch>();

beforeEach(() => {
	vi.stubGlobal('fetch', fetchMock);
	fetchMock.mockImplementation(async () => new Response(null, { status: 200 }));
});

afterEach(() => {
	fetchMock.mockReset();
	vi.unstubAllGlobals();
});

function lastCall() {
	const [url, init] = fetchMock.mock.calls.at(-1) ?? [];
	const body = init?.body;

	return {
		url,
		method: init?.method,
		json: typeof body === 'string' ? JSON.parse(body) : undefined,
		form: body instanceof FormData ? body : undefined
	};
}

const newTorrent: NewTorrent = {
	category: 'tv',
	hostDownloadAction: TorrentHostDownloadAction.DownloadAll,
	downloadAction: TorrentDownloadAction.DownloadManual,
	finishedAction: TorrentFinishedAction.RemoveClient,
	finishedActionDelay: 5,
	downloadMinSize: 10,
	includeRegex: null,
	excludeRegex: '\\.nfo$',
	downloadManualFiles: '/a.mkv,/b.mkv',
	priority: 1,
	torrentRetryAttempts: 1,
	downloadRetryAttempts: 3,
	deleteOnError: 0,
	lifetime: 0,
	downloadClient: DownloadClient.Internal
};

describe('auth', () => {
	it.each([
		['create', () => auth.create('admin', 'pass1'), 'Api/Authentication/Create'],
		['login', () => auth.login('admin', 'pass1'), 'Api/Authentication/Login'],
		['update', () => auth.update('admin', 'pass1'), 'Api/Authentication/Update']
	])('%s posts the credentials', async (_name, call, url) => {
		await call();

		expect(lastCall()).toMatchObject({
			url,
			method: 'POST',
			json: { userName: 'admin', password: 'pass1' }
		});
	});

	it('checks the session', async () => {
		await auth.isLoggedIn();

		expect(lastCall()).toMatchObject({ url: 'Api/Authentication/IsLoggedIn', method: 'GET' });
	});

	it('sets up the provider with its numeric value', async () => {
		await auth.setupProvider(Provider.TorBox, 'key');

		expect(lastCall()).toMatchObject({
			url: 'Api/Authentication/SetupProvider',
			json: { provider: 3, token: 'key' }
		});
	});

	it('logs out with an empty body', async () => {
		await auth.logout();

		expect(lastCall()).toMatchObject({
			url: 'Api/Authentication/Logout',
			method: 'POST',
			json: {}
		});
	});
});

describe('settings', () => {
	it('reads the catalogue, profile and version', async () => {
		await settings.list();
		expect(lastCall()).toMatchObject({ url: 'Api/Settings', method: 'GET' });

		await settings.profile();
		expect(lastCall().url).toBe('Api/Settings/Profile');

		await settings.version();
		expect(lastCall().url).toBe('Api/Settings/Version');
	});

	it('saves values as key/value pairs', async () => {
		await settings.update([{ key: 'General:LogLevel', value: 2 }]);
		expect(lastCall()).toMatchObject({
			url: 'Api/Settings',
			method: 'PUT',
			json: [{ key: 'General:LogLevel', value: 2 }]
		});

		await settings.updateValue('DownloadClient:MaxSpeed', 40);
		expect(lastCall().json).toEqual([{ key: 'DownloadClient:MaxSpeed', value: 40 }]);
	});

	it('runs the diagnostics', async () => {
		fetchMock.mockResolvedValueOnce(new Response('1048576', { status: 200 }));
		await expect(settings.testDownloadSpeed()).resolves.toBe(1048576);

		fetchMock.mockResolvedValueOnce(new Response('2048.5', { status: 200 }));
		await expect(settings.testWriteSpeed()).resolves.toBe(2048.5);
		expect(lastCall().url).toBe('Api/Settings/TestWriteSpeed');

		await settings.testPath('/data/downloads');
		expect(lastCall()).toMatchObject({
			url: 'Api/Settings/TestPath',
			json: { path: '/data/downloads' }
		});

		fetchMock.mockResolvedValueOnce(
			new Response('{"enabledFeatures":[],"version":"1.37.0"}', { status: 200 })
		);
		await expect(settings.testAria2cConnection('http://a:6800/jsonrpc', 's')).resolves.toEqual({
			enabledFeatures: [],
			version: '1.37.0'
		});
		expect(lastCall().json).toEqual({ url: 'http://a:6800/jsonrpc', secret: 's' });
	});
});

describe('torrents', () => {
	it('reads the list, a torrent and the monitor states', async () => {
		await torrents.list();
		expect(lastCall()).toMatchObject({ url: 'Api/Torrents', method: 'GET' });

		await torrents.get('0b9e2c1a-1111-4222-8333-444455556666');
		expect(lastCall().url).toBe('Api/Torrents/Get/0b9e2c1a-1111-4222-8333-444455556666');

		fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
		await expect(torrents.diskSpaceStatus()).resolves.toBeNull();
		expect(lastCall().url).toBe('Api/Torrents/DiskSpaceStatus');

		await torrents.rateLimitStatus();
		expect(lastCall().url).toBe('Api/Torrents/RateLimitStatus');
	});

	it('uploads a magnet with its settings', async () => {
		await torrents.uploadMagnet('magnet:?xt=urn:btih:abc', newTorrent);

		expect(lastCall()).toMatchObject({
			url: 'Api/Torrents/UploadMagnet',
			method: 'POST',
			json: { magnetLink: 'magnet:?xt=urn:btih:abc', torrent: newTorrent }
		});
	});

	it('uploads a torrent file with its settings as a form field', async () => {
		const file = new File(['d4:infoe'], 'a.torrent');

		await torrents.uploadFile(file, newTorrent);

		const { url, form } = lastCall();
		expect(url).toBe('Api/Torrents/UploadFile');
		expect((form?.get('file') as File).name).toBe('a.torrent');
		expect(JSON.parse(form?.get('formData') as string)).toEqual({ torrent: newTorrent });
	});

	it('checks the available files', async () => {
		await torrents.checkFilesMagnet('magnet:?xt=urn:btih:abc');
		expect(lastCall()).toMatchObject({
			url: 'Api/Torrents/CheckFilesMagnet',
			json: { magnetLink: 'magnet:?xt=urn:btih:abc' }
		});

		await torrents.checkFiles(new File(['x'], 'b.torrent'));
		const { url, form } = lastCall();
		expect(url).toBe('Api/Torrents/CheckFiles');
		expect(form?.has('file')).toBe(true);
		expect(form?.has('formData')).toBe(false);
	});

	it('verifies the file expressions', async () => {
		await torrents.verifyRegex('mkv$', null, 'magnet:?xt=urn:btih:abc');

		expect(lastCall()).toMatchObject({
			url: 'Api/Torrents/VerifyRegex',
			json: { includeRegex: 'mkv$', excludeRegex: null, magnetLink: 'magnet:?xt=urn:btih:abc' }
		});
	});

	it('deletes, retries and updates', async () => {
		await torrents.delete('t1', {
			deleteData: true,
			deleteRdTorrent: false,
			deleteLocalFiles: true
		});
		expect(lastCall()).toMatchObject({
			url: 'Api/Torrents/Delete/t1',
			method: 'POST',
			json: { deleteData: true, deleteRdTorrent: false, deleteLocalFiles: true }
		});

		await torrents.retry('t1');
		expect(lastCall()).toMatchObject({ url: 'Api/Torrents/Retry/t1', method: 'POST', json: {} });

		await torrents.retryDownload('d1');
		expect(lastCall()).toMatchObject({ url: 'Api/Torrents/RetryDownload/d1', json: {} });

		const torrent = { torrentId: 't1', category: 'movies', priority: 2 } as Torrent;
		await torrents.update(torrent);
		expect(lastCall()).toMatchObject({ url: 'Api/Torrents/Update', method: 'PUT', json: torrent });
	});
});

import { request } from './client.js';
import type {
	Aria2Version,
	AvailableFile,
	DeleteOptions,
	DiskSpaceStatus,
	NewTorrent,
	Profile,
	Provider,
	RateLimitStatus,
	Setting,
	SettingUpdate,
	SettingValue,
	Torrent,
	Version,
	VerifyRegexResult
} from './types.js';

export { ApiError, errorMessage, onAuthRequired, type AuthRequirement } from './client.js';

function torrentForm(file: File, torrent?: NewTorrent): FormData {
	const form = new FormData();
	form.append('file', file);

	if (torrent) {
		form.append('formData', JSON.stringify({ torrent }));
	}

	return form;
}

/** Account and session endpoints. */
export const auth = {
	/** Resolves when signed in (or authentication is disabled). */
	isLoggedIn: () => request<null>('Api/Authentication/IsLoggedIn'),
	create: (userName: string, password: string) =>
		request<null>('Api/Authentication/Create', { method: 'POST', json: { userName, password } }),
	setupProvider: (provider: Provider, token: string) =>
		request<null>('Api/Authentication/SetupProvider', {
			method: 'POST',
			json: { provider, token }
		}),
	login: (userName: string, password: string) =>
		request<null>('Api/Authentication/Login', { method: 'POST', json: { userName, password } }),
	logout: () => request<null>('Api/Authentication/Logout', { method: 'POST', json: {} }),
	update: (userName: string, password: string) =>
		request<null>('Api/Authentication/Update', { method: 'POST', json: { userName, password } })
};

/** Settings, profile and diagnostics endpoints. */
export const settings = {
	list: () => request<Setting[]>('Api/Settings'),
	update: (values: SettingUpdate[]) =>
		request<null>('Api/Settings', { method: 'PUT', json: values }),
	updateValue: (key: string, value: SettingValue) =>
		request<null>('Api/Settings', { method: 'PUT', json: [{ key, value }] }),
	profile: () => request<Profile>('Api/Settings/Profile'),
	version: () => request<Version>('Api/Settings/Version'),
	testPath: (path: string) =>
		request<null>('Api/Settings/TestPath', { method: 'POST', json: { path } }),
	/** Resolves with the measured download speed in bytes per second. */
	testDownloadSpeed: () => request<number>('Api/Settings/TestDownloadSpeed'),
	/** Resolves with the measured write speed in bytes per second. */
	testWriteSpeed: () => request<number>('Api/Settings/TestWriteSpeed'),
	testAria2cConnection: (url: string, secret: string) =>
		request<Aria2Version>('Api/Settings/TestAria2cConnection', {
			method: 'POST',
			json: { url, secret }
		})
};

/** Torrent endpoints. */
export const torrents = {
	list: () => request<Torrent[]>('Api/Torrents'),
	get: (torrentId: string) => request<Torrent>(`Api/Torrents/Get/${encodeURIComponent(torrentId)}`),
	/** Resolves with `null` while the disk space monitor has nothing to report. */
	diskSpaceStatus: () => request<DiskSpaceStatus | null>('Api/Torrents/DiskSpaceStatus'),
	rateLimitStatus: () => request<RateLimitStatus | null>('Api/Torrents/RateLimitStatus'),
	uploadMagnet: (magnetLink: string, torrent: NewTorrent) =>
		request<null>('Api/Torrents/UploadMagnet', { method: 'POST', json: { magnetLink, torrent } }),
	uploadFile: (file: File, torrent: NewTorrent) =>
		request<null>('Api/Torrents/UploadFile', { method: 'POST', form: torrentForm(file, torrent) }),
	checkFilesMagnet: (magnetLink: string) =>
		request<AvailableFile[]>('Api/Torrents/CheckFilesMagnet', {
			method: 'POST',
			json: { magnetLink }
		}),
	checkFiles: (file: File) =>
		request<AvailableFile[]>('Api/Torrents/CheckFiles', {
			method: 'POST',
			form: torrentForm(file)
		}),
	verifyRegex: (includeRegex: string | null, excludeRegex: string | null, magnetLink: string) =>
		request<VerifyRegexResult | null>('Api/Torrents/VerifyRegex', {
			method: 'POST',
			json: { includeRegex, excludeRegex, magnetLink }
		}),
	delete: (torrentId: string, options: DeleteOptions) =>
		request<null>(`Api/Torrents/Delete/${encodeURIComponent(torrentId)}`, {
			method: 'POST',
			json: options
		}),
	retry: (torrentId: string) =>
		request<null>(`Api/Torrents/Retry/${encodeURIComponent(torrentId)}`, {
			method: 'POST',
			json: {}
		}),
	retryDownload: (downloadId: string) =>
		request<null>(`Api/Torrents/RetryDownload/${encodeURIComponent(downloadId)}`, {
			method: 'POST',
			json: {}
		}),
	/** Saves a torrent's settings; the server persists only the `TorrentSettings` fields. */
	update: (torrent: Torrent) =>
		request<null>('Api/Torrents/Update', { method: 'PUT', json: torrent })
};

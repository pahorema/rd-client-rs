/** RFC 3339 timestamp with offset, as sent by the server. */
export type Timestamp = string;

/** Status of a torrent on the debrid provider. */
export const TorrentStatus = {
	Queued: 0,
	Processing: 1,
	WaitingForFileSelection: 2,
	Downloading: 3,
	Finished: 4,
	Uploading: 5,
	Error: 99
} as const;
export type TorrentStatus = (typeof TorrentStatus)[keyof typeof TorrentStatus];

/** Debrid providers, in the numeric order used by `SetupProvider`. */
export const Provider = {
	RealDebrid: 0,
	AllDebrid: 1,
	Premiumize: 2,
	TorBox: 3,
	DebridLink: 4,
	Highway: 5
} as const;
export type Provider = (typeof Provider)[keyof typeof Provider];

/** Downloader used to transfer finished files from the provider to the host. */
export const DownloadClient = {
	Internal: 0,
	Aria2c: 1,
	Symlink: 2,
	DownloadStation: 3
} as const;
export type DownloadClient = (typeof DownloadClient)[keyof typeof DownloadClient];

/** Which files of a torrent are selected on the provider. */
export const TorrentDownloadAction = {
	DownloadAll: 0,
	DownloadAvailableFiles: 1,
	DownloadManual: 2
} as const;
export type TorrentDownloadAction =
	(typeof TorrentDownloadAction)[keyof typeof TorrentDownloadAction];

/** What happens once every file is downloaded to the host. */
export const TorrentFinishedAction = {
	None: 0,
	RemoveAllTorrents: 1,
	RemoveProvider: 2,
	RemoveClient: 3
} as const;
export type TorrentFinishedAction =
	(typeof TorrentFinishedAction)[keyof typeof TorrentFinishedAction];

/** Whether files finished on the provider are downloaded to the host. */
export const TorrentHostDownloadAction = {
	DownloadAll: 0,
	DownloadNone: 1
} as const;
export type TorrentHostDownloadAction =
	(typeof TorrentHostDownloadAction)[keyof typeof TorrentHostDownloadAction];

/** A file of a torrent as reported by the debrid provider. */
export interface TorrentFile {
	id: number;
	path: string;
	bytes: number;
	selected: boolean;
	downloadLink: string | null;
}

/** One file transfer from the provider to the host, with live statistics. */
export interface Download {
	downloadId: string;
	torrentId: string;
	/** Restricted provider link. */
	path: string;
	/** Unrestricted link, once generated. */
	link: string | null;
	added: Timestamp;
	downloadQueued: Timestamp | null;
	downloadStarted: Timestamp | null;
	downloadFinished: Timestamp | null;
	unpackingQueued: Timestamp | null;
	unpackingStarted: Timestamp | null;
	unpackingFinished: Timestamp | null;
	completed: Timestamp | null;
	retryCount: number;
	error: string | null;
	bytesTotal: number;
	bytesDone: number;
	speed: number;
}

/**
 * A torrent as returned by the server.
 *
 * The list endpoint leaves `downloads`, `files` and `fileOrMagnet` empty, live
 * updates fill `downloads`, and the detail endpoint fills all three.
 */
export interface Torrent {
	torrentId: string;
	hash: string;
	category: string | null;
	downloadAction: TorrentDownloadAction;
	finishedAction: TorrentFinishedAction;
	finishedActionDelay: number;
	hostDownloadAction: TorrentHostDownloadAction;
	downloadMinSize: number;
	includeRegex: string | null;
	excludeRegex: string | null;
	downloadManualFiles: string | null;
	downloadClient: DownloadClient;
	added: Timestamp;
	filesSelected: Timestamp | null;
	completed: Timestamp | null;
	/** Base64 torrent file or magnet link (detail only). */
	fileOrMagnet: string | null;
	isFile: boolean;
	priority: number | null;
	retryCount: number;
	downloadRetryAttempts: number;
	torrentRetryAttempts: number;
	deleteOnError: number;
	lifetime: number;
	error: string | null;
	rdId: string | null;
	rdName: string | null;
	rdSize: number | null;
	rdHost: string | null;
	rdSplit: number | null;
	rdProgress: number | null;
	rdStatus: TorrentStatus | null;
	rdStatusRaw: string | null;
	rdAdded: Timestamp | null;
	rdEnded: Timestamp | null;
	rdSpeed: number | null;
	rdSeeders: number | null;
	statusText: string;
	filesCount: number;
	downloadsCount: number;
	files: TorrentFile[];
	downloads: Download[];
}

/** Per-torrent settings sent when adding a torrent. */
export interface NewTorrent {
	category: string | null;
	hostDownloadAction: TorrentHostDownloadAction;
	downloadAction: TorrentDownloadAction;
	finishedAction: TorrentFinishedAction;
	finishedActionDelay: number;
	downloadMinSize: number;
	includeRegex: string | null;
	excludeRegex: string | null;
	/** Comma-separated provider file paths, only for manual selection. */
	downloadManualFiles: string | null;
	priority: number | null;
	torrentRetryAttempts: number;
	downloadRetryAttempts: number;
	deleteOnError: number;
	lifetime: number;
	downloadClient: DownloadClient;
}

/** The torrent settings the server persists on `PUT Api/Torrents/Update`. */
export type TorrentSettings = Pick<
	Torrent,
	| 'downloadClient'
	| 'hostDownloadAction'
	| 'category'
	| 'priority'
	| 'downloadRetryAttempts'
	| 'torrentRetryAttempts'
	| 'deleteOnError'
	| 'lifetime'
>;

/** A file the provider reports as available for a torrent. */
export interface AvailableFile {
	filename: string;
	filesize: number;
}

/** Result of checking include/exclude expressions against the available files. */
export interface VerifyRegexResult {
	includeError: string;
	excludeError: string;
	selectedFiles: AvailableFile[];
}

/** Which parts of a torrent to remove. */
export interface DeleteOptions {
	/** Remove the torrent from the client. */
	deleteData: boolean;
	/** Remove the torrent from the debrid provider. */
	deleteRdTorrent: boolean;
	/** Remove the downloaded files from the host. */
	deleteLocalFiles: boolean;
}

/** State of the low disk space monitor. */
export interface DiskSpaceStatus {
	isPaused: boolean;
	availableSpaceGB: number;
	thresholdGB: number;
	lastCheckTime: Timestamp;
}

/** State of the provider rate limit cooldown. */
export interface RateLimitStatus {
	nextDequeueTime: Timestamp | null;
	secondsRemaining: number;
}

/** Provider account and version information. */
export interface Profile {
	/** Provider name, e.g. `RealDebrid`. */
	provider: string | null;
	userName: string | null;
	expiration: Timestamp | null;
	premium: boolean;
	currentVersion: string | null;
	latestVersion: string | null;
	isInsecure: boolean | null;
	disableUpdateNotification: boolean | null;
}

/** Server build version. */
export interface Version {
	version: string;
}

/** Version information of a reachable aria2 instance. */
export interface Aria2Version {
	version: string;
	enabledFeatures: string[];
}

export type SettingType = 'Object' | 'Enum' | 'Boolean' | 'Int32' | 'String';
export type SettingValue = string | number | boolean | null;

/**
 * One entry of the settings catalogue. `Object` entries are group headings
 * (tabs when the key has no `:`) and carry no value.
 */
export interface Setting {
	key: string;
	value: SettingValue;
	displayName: string | null;
	description: string | null;
	type: SettingType;
	/** Option labels keyed by the numeric value, for `Enum` settings. */
	enumValues: Record<string, string> | null;
}

/** Body of `PUT Api/Settings`: only the key and value are read. */
export interface SettingUpdate {
	key: string;
	value: SettingValue;
}

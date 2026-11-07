import { auth, torrents as torrentsApi } from './api/index.js';
import type { DiskSpaceStatus, RateLimitStatus, Torrent } from './api/types.js';
import { connectEvents } from './events.js';

/**
 * Live server state pushed over the event stream. Every update replaces a value whole, so
 * the values are kept raw instead of being proxied deeply on each push.
 */
class LiveState {
	/** Latest torrent list with download statistics; `null` until the first update. */
	torrents = $state.raw<Torrent[] | null>(null);
	diskSpace = $state.raw<DiskSpaceStatus | null>(null);
	rateLimit = $state.raw<RateLimitStatus | null>(null);
	connected = $state(false);
}

export const live = new LiveState();

/**
 * Loads both monitor states. The stream only pushes them when they change, so a change
 * missed while disconnected is only seen this way.
 */
function refreshMonitors() {
	torrentsApi
		.diskSpaceStatus()
		.then((status) => (live.diskSpace = status))
		.catch(() => undefined);
	torrentsApi
		.rateLimitStatus()
		.then((status) => (live.rateLimit = status))
		.catch(() => undefined);
}

/**
 * Loads the monitor states and subscribes to the event stream.
 * Returns a function that unsubscribes and clears the state.
 */
export function startLive(): () => void {
	refreshMonitors();

	const stop = connectEvents({
		update: (torrents) => (live.torrents = torrents),
		diskSpaceStatus: (status) => (live.diskSpace = status),
		rateLimitStatus: (status) => (live.rateLimit = status),
		reconnected: refreshMonitors,
		connectionChange: (connected) => (live.connected = connected),
		// A refused stream usually means the session expired; this request
		// sends the user to the login or setup page when that is the case.
		rejected: () => void auth.isLoggedIn().catch(() => undefined)
	});

	return () => {
		stop();
		live.torrents = null;
		live.diskSpace = null;
		live.rateLimit = null;
		live.connected = false;
	};
}

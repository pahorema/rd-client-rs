import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EventHandlers } from './events.js';
import { live, startLive } from './live.svelte.js';

const api = vi.hoisted(() => ({
	isLoggedIn: vi.fn(),
	diskSpaceStatus: vi.fn(),
	rateLimitStatus: vi.fn()
}));
const stream = vi.hoisted(() => ({ handlers: null as EventHandlers | null, stop: vi.fn() }));

vi.mock('./api/index.js', () => ({
	auth: { isLoggedIn: api.isLoggedIn },
	torrents: { diskSpaceStatus: api.diskSpaceStatus, rateLimitStatus: api.rateLimitStatus }
}));
vi.mock('./events.js', () => ({
	connectEvents: (handlers: EventHandlers) => {
		stream.handlers = handlers;
		return stream.stop;
	}
}));

const paused = { isPaused: true, availableSpaceGB: 3, thresholdGB: 10, lastCheckTime: 'x' };
const limited = { nextDequeueTime: '2030-01-01T00:00:00+00:00', secondsRemaining: 60 };

beforeEach(() => {
	api.isLoggedIn.mockResolvedValue(null);
	api.diskSpaceStatus.mockResolvedValue(null);
	api.rateLimitStatus.mockResolvedValue(null);
});

afterEach(() => {
	vi.clearAllMocks();
});

describe('startLive', () => {
	it('loads the monitor states', async () => {
		api.diskSpaceStatus.mockResolvedValue(paused);
		api.rateLimitStatus.mockResolvedValue(limited);

		const stop = startLive();
		await vi.waitFor(() => expect(live.rateLimit).toEqual(limited));

		expect(live.diskSpace).toEqual(paused);
		stop();
	});

	it('applies pushed events', () => {
		const stop = startLive();
		const handlers = stream.handlers!;

		handlers.update([{ torrentId: 't1' } as never]);
		handlers.diskSpaceStatus(paused);
		handlers.rateLimitStatus(limited);
		handlers.connectionChange(true);

		expect(live.torrents).toEqual([{ torrentId: 't1' }]);
		expect(live.diskSpace).toEqual(paused);
		expect(live.rateLimit).toEqual(limited);
		expect(live.connected).toBe(true);
		stop();
	});

	it('refreshes both monitor states after reconnecting', async () => {
		const stop = startLive();
		api.diskSpaceStatus.mockResolvedValue(paused);
		// A cooldown that started while disconnected is never pushed afterwards.
		api.rateLimitStatus.mockResolvedValue(limited);

		stream.handlers!.reconnected();

		await vi.waitFor(() => expect(live.rateLimit).toEqual(limited));
		expect(live.diskSpace).toEqual(paused);
		expect(api.diskSpaceStatus).toHaveBeenCalledTimes(2);
		expect(api.rateLimitStatus).toHaveBeenCalledTimes(2);
		stop();
	});

	it('rechecks the session when the stream is refused', () => {
		const stop = startLive();

		stream.handlers!.rejected();

		expect(api.isLoggedIn).toHaveBeenCalledTimes(1);
		stop();
	});

	it('clears the state when stopped', () => {
		const stop = startLive();
		stream.handlers!.update([]);
		stream.handlers!.connectionChange(true);

		stop();

		expect(stream.stop).toHaveBeenCalledTimes(1);
		expect(live.torrents).toBeNull();
		expect(live.connected).toBe(false);
	});
});

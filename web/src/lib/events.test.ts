import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { connectEvents, type EventHandlers, type EventSourceLike } from './events.js';

class FakeEventSource implements EventSourceLike {
	readyState = 0;
	closed = false;
	private listeners = new Map<string, ((event: MessageEvent) => void)[]>();

	constructor(readonly url: string) {}

	addEventListener(type: string, listener: (event: MessageEvent) => void) {
		this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
	}

	close() {
		this.closed = true;
		this.readyState = 2;
	}

	emit(type: string, data?: string) {
		for (const listener of this.listeners.get(type) ?? []) {
			listener(new MessageEvent(type, { data }));
		}
	}

	open() {
		this.readyState = 1;
		this.emit('open');
	}

	fail(readyState: number) {
		this.readyState = readyState;
		this.emit('error');
	}
}

let sources: FakeEventSource[];
let handlers: { [K in keyof EventHandlers]: ReturnType<typeof vi.fn> };

function connect() {
	return connectEvents(handlers as unknown as EventHandlers, {
		create: (url) => {
			const source = new FakeEventSource(url);
			sources.push(source);
			return source;
		},
		retryDelay: 100,
		maxRetryDelay: 300
	});
}

beforeEach(() => {
	vi.useFakeTimers();
	sources = [];
	handlers = {
		update: vi.fn(),
		diskSpaceStatus: vi.fn(),
		rateLimitStatus: vi.fn(),
		reconnected: vi.fn(),
		connectionChange: vi.fn(),
		rejected: vi.fn()
	};
});

afterEach(() => {
	vi.useRealTimers();
});

describe('connectEvents', () => {
	it('opens the relative event stream', () => {
		connect();

		expect(sources.map((s) => s.url)).toEqual(['Api/Events']);
	});

	it('dispatches the named events with parsed data', () => {
		connect();
		const [source] = sources;

		source.emit('update', '[{"torrentId":"t1"}]');
		source.emit('diskSpaceStatus', '{"isPaused":true}');
		source.emit('rateLimitStatus', '{"nextDequeueTime":null,"secondsRemaining":0}');

		expect(handlers.update).toHaveBeenCalledWith([{ torrentId: 't1' }]);
		expect(handlers.diskSpaceStatus).toHaveBeenCalledWith({ isPaused: true });
		expect(handlers.rateLimitStatus).toHaveBeenCalledWith({
			nextDequeueTime: null,
			secondsRemaining: 0
		});
	});

	it('ignores malformed event data', () => {
		connect();

		sources[0].emit('update', 'not json');

		expect(handlers.update).not.toHaveBeenCalled();
	});

	it('reports reconnection only after a previous connection', () => {
		connect();
		const [source] = sources;

		source.open();
		expect(handlers.reconnected).not.toHaveBeenCalled();

		source.fail(0);
		source.open();

		expect(handlers.reconnected).toHaveBeenCalledTimes(1);
		expect(handlers.connectionChange.mock.calls).toEqual([[true], [false], [true]]);
		expect(sources).toHaveLength(1);
	});

	it('reopens a refused stream with growing delays', () => {
		connect();

		sources[0].fail(2);
		expect(handlers.rejected).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(99);
		expect(sources).toHaveLength(1);
		vi.advanceTimersByTime(1);
		expect(sources).toHaveLength(2);

		sources[1].fail(2);
		vi.advanceTimersByTime(199);
		expect(sources).toHaveLength(2);
		vi.advanceTimersByTime(1);
		expect(sources).toHaveLength(3);

		sources[2].fail(2);
		vi.advanceTimersByTime(300);
		expect(sources).toHaveLength(4);

		sources[3].open();
		expect(handlers.reconnected).not.toHaveBeenCalled();
	});

	it('resets the delay once a stream opens', () => {
		connect();

		sources[0].fail(2);
		vi.advanceTimersByTime(100);
		sources[1].open();
		sources[1].fail(2);
		vi.advanceTimersByTime(100);

		expect(sources).toHaveLength(3);
		sources[2].open();
		expect(handlers.reconnected).toHaveBeenCalledTimes(1);
	});

	it('stops retrying once disconnected', () => {
		const disconnect = connect();

		sources[0].fail(2);
		disconnect();
		vi.advanceTimersByTime(1000);

		expect(sources).toHaveLength(1);
		expect(sources[0].closed).toBe(true);
	});
});

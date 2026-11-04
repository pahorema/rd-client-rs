import type { DiskSpaceStatus, RateLimitStatus, Torrent } from './api/types.js';

/** Callbacks for the server event stream. */
export interface EventHandlers {
	/** Full torrent list with live download statistics. */
	update(torrents: Torrent[]): void;
	diskSpaceStatus(status: DiskSpaceStatus): void;
	rateLimitStatus(status: RateLimitStatus): void;
	/** The stream is open again after having been interrupted. */
	reconnected(): void;
	/** The stream opened or was interrupted. */
	connectionChange(connected: boolean): void;
	/** The server refused the stream (e.g. the session expired); a retry is scheduled. */
	rejected(): void;
}

/** The subset of `EventSource` the stream needs, so tests can substitute it. */
export interface EventSourceLike {
	readonly readyState: number;
	addEventListener(type: string, listener: (event: MessageEvent) => void): void;
	close(): void;
}

export interface EventStreamOptions {
	url?: string;
	create?: (url: string) => EventSourceLike;
	/** First delay before reopening a stream the server closed, doubled up to `maxRetryDelay`. */
	retryDelay?: number;
	maxRetryDelay?: number;
}

const CLOSED = 2;

/**
 * Subscribes to `Api/Events`. The browser resumes interrupted streams by
 * itself; streams the server refused are reopened with exponential backoff.
 * Returns a function that closes the stream.
 */
export function connectEvents(handlers: EventHandlers, options: EventStreamOptions = {}) {
	const url = options.url ?? 'Api/Events';
	const create = options.create ?? ((target: string) => new EventSource(target));
	const initialDelay = options.retryDelay ?? 2000;
	const maxDelay = options.maxRetryDelay ?? 30000;

	let source: EventSourceLike | null = null;
	let timer: ReturnType<typeof setTimeout> | undefined;
	let wasOpen = false;
	let delay = initialDelay;
	let stopped = false;

	function listen<T>(target: EventSourceLike, type: string, handle: (data: T) => void) {
		target.addEventListener(type, (event) => {
			let data: T;

			try {
				data = JSON.parse(event.data) as T;
			} catch {
				return;
			}

			handle(data);
		});
	}

	function open() {
		const current = create(url);
		source = current;

		current.addEventListener('open', () => {
			delay = initialDelay;
			handlers.connectionChange(true);

			if (wasOpen) {
				handlers.reconnected();
			}

			wasOpen = true;
		});

		current.addEventListener('error', () => {
			handlers.connectionChange(false);

			if (current.readyState !== CLOSED || stopped) {
				return;
			}

			handlers.rejected();
			timer = setTimeout(() => {
				if (!stopped) {
					open();
				}
			}, delay);
			delay = Math.min(delay * 2, maxDelay);
		});

		listen(current, 'update', handlers.update);
		listen(current, 'diskSpaceStatus', handlers.diskSpaceStatus);
		listen(current, 'rateLimitStatus', handlers.rateLimitStatus);
	}

	open();

	return () => {
		stopped = true;
		clearTimeout(timer);
		source?.close();
	};
}

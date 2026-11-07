import { errorMessage } from './api/index.js';
import type { AvailableFile } from './api/types.js';

/**
 * The files the provider reports for the torrent being added. Each magnet link or torrent
 * file is checked once, and only the reply for the current one is kept: a slow reply for a
 * link the user already replaced never shows up or changes the state.
 */
export class FileCheck {
	/** Files of the current source; `null` while unknown. */
	available = $state.raw<AvailableFile[] | null>(null);
	checking = $state(false);
	error = $state<string | null>(null);

	#source: string | null = null;
	#pending: Promise<void> = Promise.resolve();
	readonly #loaded: (files: AvailableFile[]) => void;

	/** `loaded` runs with the files of each current source once they arrive. */
	constructor(loaded: (files: AvailableFile[]) => void) {
		this.#loaded = loaded;
	}

	/**
	 * Checks `source` (an empty string for nothing to check) with `load`, unless it is the
	 * source already checked. Resolves when the check of the current source has settled.
	 */
	check(source: string, load: () => Promise<AvailableFile[] | null>): Promise<void> {
		if (source === this.#source) {
			return this.#pending;
		}

		this.#source = source;
		this.available = null;
		this.error = null;
		this.checking = source !== '';
		this.#pending = source ? this.#run(source, load) : Promise.resolve();

		return this.#pending;
	}

	async #run(source: string, load: () => Promise<AvailableFile[] | null>) {
		try {
			const files = (await load()) ?? [];

			if (source === this.#source) {
				this.available = files;
				this.checking = false;
				this.#loaded(files);
			}
		} catch (e) {
			if (source === this.#source) {
				this.error = errorMessage(e);
				this.checking = false;
				// Checking the same source again retries it.
				this.#source = null;
			}
		}
	}
}

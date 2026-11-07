/**
 * Numbers overlapping requests of one kind. `next()` starts a request and returns a check
 * telling whether it is still the most recent one, so replies to superseded requests that
 * arrive late can be dropped.
 */
export class Latest {
	#current = 0;

	next(): () => boolean {
		const id = ++this.#current;

		return () => id === this.#current;
	}
}

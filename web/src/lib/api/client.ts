/** Error returned by the server; `message` is the plain-text response body when present. */
export class ApiError extends Error {
	readonly status: number;

	constructor(status: number, message: string) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
	}
}

/** Where the user must go after the server rejected a request. */
export type AuthRequirement = 'setup' | 'login';

let authRequiredHandler: ((requirement: AuthRequirement) => void) | null = null;

/**
 * Registers the callback invoked when a request fails because the client is
 * not set up yet (402) or the user is not signed in (401/403).
 */
export function onAuthRequired(handler: ((requirement: AuthRequirement) => void) | null): void {
	authRequiredHandler = handler;
}

export interface RequestOptions {
	method?: 'GET' | 'POST' | 'PUT';
	/** Serialized as a JSON request body. */
	json?: unknown;
	/** Sent as a multipart request body. */
	form?: FormData;
	signal?: AbortSignal;
}

/** Extracts a user-facing message from any thrown value. */
export function errorMessage(error: unknown): string {
	if (error instanceof Error) {
		return error.message;
	}

	return String(error);
}

async function failure(response: Response): Promise<ApiError> {
	const body = (await response.text().catch(() => '')).trim();
	const fallback = `Request failed with status ${response.status}${response.statusText ? ` ${response.statusText}` : ''}`;

	return new ApiError(response.status, body || fallback);
}

/**
 * Sends a request to `path`, which is resolved against the document base so
 * the UI works under any base path. Resolves with the parsed JSON body, or
 * `null` when the response has no body.
 */
export async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
	const headers: Record<string, string> = { Accept: 'application/json, text/plain, */*' };
	let body: BodyInit | undefined;

	if (options.json !== undefined) {
		headers['Content-Type'] = 'application/json';
		body = JSON.stringify(options.json);
	} else if (options.form) {
		body = options.form;
	}

	const response = await fetch(path, {
		method: options.method ?? 'GET',
		headers,
		body,
		credentials: 'same-origin',
		signal: options.signal
	});

	if (!response.ok) {
		if (response.status === 402) {
			authRequiredHandler?.('setup');
		} else if (response.status === 401 || response.status === 403) {
			authRequiredHandler?.('login');
		}

		throw await failure(response);
	}

	const text = await response.text();

	if (text.length === 0) {
		return null as T;
	}

	try {
		return JSON.parse(text) as T;
	} catch {
		throw new ApiError(response.status, `Unexpected response from the server: ${text}`);
	}
}

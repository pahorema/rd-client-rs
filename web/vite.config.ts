import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			adapter: adapter({ pages: 'build', assets: 'build', fallback: undefined }),
			// The UI is served by the backend under an arbitrary base path, so routing
			// lives in the URL fragment and every asset is addressed relatively.
			router: { type: 'hash' },
			paths: { relative: true }
		})
	],
	server: {
		proxy: {
			'/Api': { target: 'http://localhost:6500', changeOrigin: true }
		}
	}
});

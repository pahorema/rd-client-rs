/// <reference types="bun" />

// The hash-router build renders `index.html` with root-absolute `/_app/…` URLs.
// The backend may serve the UI under any base path, and the document is always
// loaded from that base, so rewrite those URLs to be relative to the document.
const file = Bun.file(new URL('../build/index.html', import.meta.url));
const html = await file.text();
const rewritten = html.replace(/(["'(])\/_app\//g, '$1./_app/');

await Bun.write(file, rewritten);

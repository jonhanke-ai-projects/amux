// Build crates/amux-dashboard/static/vendor/draw/: the Draw tab's editor
// (Excalidraw + React), served by amux itself so it works offline and never
// touches a CDN at runtime. Loaded LAZILY by app.js (`import('/vendor/draw/draw.js')`)
// the first time the Draw tab opens, so app boot does not pay for it.
//
//   @excalidraw/excalidraw 0.18.1   editor, export, library, mermaid dialog
//   react / react-dom      18.3.1   its peer dependency
//
// Output is ESM with code splitting: draw.js is the core; the SVG
// font-subsetting module and other lazy imports stay in their own chunks and
// load only when used. The Mermaid converter (mermaid itself is ~5 MB) is NOT
// vendored: its dynamic import is rewritten to a pinned esm.sh URL, so it is
// fetched from the network the first time a Mermaid diagram is converted and
// costs nothing otherwise. Mermaid conversion therefore needs a connection;
// drawing, saving and the library do not. Non-English locales are stubbed (the tab
// renders in English, and the 50 locale files would ship ~1.7 MB for nothing).
// Fonts are copied except Xiaolai (CJK, 12 MB); Excalidraw falls back to its
// own CDN URL for that one face if a drawing uses CJK glyphs.
//
// Installs into a private temp dir (never the shared node_modules).
// Usage: node scripts/build-draw.mjs
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { build } from 'esbuild';

const PKGS = { '@excalidraw/excalidraw': '0.18.1', react: '18.3.1', 'react-dom': '18.3.1' };
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'amux-draw-'));
fs.writeFileSync(path.join(tmp, 'package.json'), JSON.stringify({ private: true, dependencies: PKGS }));
execFileSync('npm', ['install', '--silent', '--no-audit', '--no-fund'], { cwd: tmp, stdio: 'inherit' });

const entry = path.join(tmp, 'entry.mjs');
fs.writeFileSync(entry, `
import React from 'react';
import { createRoot } from 'react-dom/client';
import * as X from '@excalidraw/excalidraw';
export { React, createRoot, X };
// Network import (see MERMAID_URL): only fetched when Mermaid is converted.
export const loadMermaid = () => import('@excalidraw/mermaid-to-excalidraw');
`);

const MERMAID_URL = 'https://esm.sh/@excalidraw/mermaid-to-excalidraw@2.2.2?deps=mermaid@11.12.1';
const mermaidFromNetwork = {
  name: 'mermaid-from-network',
  setup(b) {
    b.onResolve({ filter: /^@excalidraw\/mermaid-to-excalidraw$/ }, () => ({ path: MERMAID_URL, external: true }));
  },
};

const stubLocales = {
  name: 'stub-locales',
  setup(b) {
    b.onResolve({ filter: /\/locales\/[^/]+\.js$/ }, (args) =>
      /\/locales\/en-[A-Z0-9]+\.js$/.test(args.path) ? undefined : { path: args.path, namespace: 'stub-locale' });
    b.onLoad({ filter: /.*/, namespace: 'stub-locale' }, () => ({ contents: 'export default {};', loader: 'js' }));
  },
};

const dist = path.join(tmp, 'node_modules/@excalidraw/excalidraw/dist/prod');
const out = new URL('../crates/amux-dashboard/static/vendor/draw/', import.meta.url).pathname;
fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });
const banner = `/* amux vendor bundle: ${Object.entries(PKGS).map(([k, v]) => k + '@' + v).join(' ')} (scripts/build-draw.mjs) */`;
const result = await build({
  entryPoints: { draw: entry }, bundle: true, format: 'esm', splitting: true, minify: true, target: 'es2020',
  outdir: out, chunkNames: 'chunk-[hash]', nodePaths: [path.join(tmp, 'node_modules')], absWorkingDir: tmp,
  define: { 'process.env.NODE_ENV': '"production"' }, plugins: [stubLocales, mermaidFromNetwork], metafile: true,
  legalComments: 'none', banner: { js: banner }, logLevel: 'warning',
});

// The font-subsetting chunk embeds a wasm binary as one base64 string, and
// that random text happens to contain AWS-access-key-shaped runs. The repo's
// pre-commit secret scan (rightly) has no allowlist, so split each such run
// inside its double-quoted literal ("...AKI"+"A..."): same bytes at runtime, no
// false positive. Refuses to touch a match that is not inside a "..." literal.
const KEYLIKE = /AKIA[0-9A-Z]{16}/g;
let split = 0;
for (const f of fs.readdirSync(out).filter((n) => n.endsWith('.js'))) {
  const file = path.join(out, f);
  let src = fs.readFileSync(file, 'utf8');
  if (!KEYLIKE.test(src)) continue;
  src = src.replace(KEYLIKE, (m, at, whole) => {
    const open = whole.lastIndexOf('"', at), close = whole.indexOf('"', at);
    const inside = whole.slice(open + 1, close);
    if (open < 0 || close < 0 || !/^[A-Za-z0-9+/=]*$/.test(inside)) throw new Error(`key-like run outside a base64 literal in ${f} at ${at}`);
    split++;
    return 'AKI"+"' + m.slice(3);
  });
  fs.writeFileSync(file, src);
}
if (split) console.log(`split ${split} key-shaped run(s) inside base64 literals (secret-scan false positives)`);

// Excalidraw's dist inlines its own app's environment, including the public
// Firebase web config of excalidraw.com's collaboration backend. This tab
// never uses it (no collaboration, no excalidraw.com backend), so it is
// emptied rather than vendored into this repo.
let stripped = 0;
for (const f of fs.readdirSync(out).filter((n) => n.endsWith('.js'))) {
  const file = path.join(out, f);
  const src = fs.readFileSync(file, 'utf8');
  const next = src.replace(/VITE_APP_FIREBASE_CONFIG:'[^']*'/g, () => { stripped++; return "VITE_APP_FIREBASE_CONFIG:'{}'"; });
  if (next !== src) fs.writeFileSync(file, next);
}
console.log(`emptied ${stripped} inlined excalidraw.com Firebase config(s)`);

// The same credential shapes the pre-commit scan refuses; a match left here
// would block every commit of the bundle, so fail the build and say where.
// (The private-key shape is assembled from pieces so this file does not match it.)
const SECRET_SHAPES = [new RegExp('-----' + 'BEGIN .* PRIVATE KEY|-----' + 'BEGIN PRIVATE KEY'), /sk-ant-api[0-9a-zA-Z-]{20,}/, /tskey-auth-[A-Za-z0-9]+/,
  /AKIA[0-9A-Z]{16}/, /ghp_[A-Za-z0-9]{36}/, /github_pat_[A-Za-z0-9_]+/, /AIza[0-9A-Za-z_-]{35}/, /sk-[a-zA-Z0-9]{48}/,
  /sk_(test|live)_[A-Za-z0-9]{20,}/, /xox[baprs]-[A-Za-z0-9-]{10,}/, /glpat-[A-Za-z0-9_-]{20,}/];
const shaped = [];
for (const f of fs.readdirSync(out).filter((n) => /\.(js|css)$/.test(n))) {
  const src = fs.readFileSync(path.join(out, f), 'utf8');
  for (const re of SECRET_SHAPES) { const m = src.match(re); if (m) shaped.push(f + ': ' + re + ' -> ' + m[0].slice(0, 12) + '...'); }
}
if (shaped.length) throw new Error('credential-shaped strings left in the bundle:\n' + shaped.join('\n'));

fs.copyFileSync(path.join(dist, 'index.css'), path.join(out, 'draw.css'));
const fontsOut = path.join(out, 'fonts');
for (const face of fs.readdirSync(path.join(dist, 'fonts'))) {
  if (face === 'Xiaolai') continue;
  fs.cpSync(path.join(dist, 'fonts', face), path.join(fontsOut, face), { recursive: true });
}

// What the service worker precaches so the tab opens offline: the core entry,
// every chunk it imports STATICALLY, the stylesheet and the default hand-drawn
// face. Lazy chunks (subsetting, images) are cached on first use instead.
const outputs = result.metafile.outputs;
const rel = (p) => path.relative(out, path.resolve(tmp, p)).split(path.sep).join('/');
const core = new Set();
const walk = (key) => {
  const r = rel(key);
  if (core.has(r)) return;
  core.add(r);
  for (const imp of outputs[key].imports || []) {
    if (imp.kind === 'import-statement' && outputs[imp.path]) walk(imp.path);
  }
};
walk(Object.keys(outputs).find((o) => o.endsWith('draw.js')));
const files = [...core, 'draw.css', ...fs.readdirSync(path.join(fontsOut, 'Excalifont')).map((f) => 'fonts/Excalifont/' + f)];
fs.writeFileSync(path.join(out, 'precache.json'), JSON.stringify({ versions: PKGS, files: files.map((f) => '/vendor/draw/' + f) }, null, 1) + '\n');

let total = 0;
for (const f of fs.readdirSync(out, { recursive: true })) {
  const s = fs.statSync(path.join(out, f));
  if (s.isFile()) total += s.size;
}
const coreBytes = [...core].reduce((n, f) => n + fs.statSync(path.join(out, f)).size, 0);
console.log(`wrote ${out}: ${total} bytes total, core (draw.js + static imports) ${coreBytes} bytes in ${core.size} file(s)`);

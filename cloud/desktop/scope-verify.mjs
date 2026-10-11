// End-to-end check that a group-scoped account sees its group and nothing else.
//
// Signed in as SCOPED (e.g. elliot, provisioned with group=wexus) through the
// host's sign-in form, in a real browser, and over the API; GLOBAL (e.g. ethan)
// is the control that must still see everything.
//
//   AMUX_REMOTE=https://sandbox.amux.io GROUP=wexus \
//   SCOPED=elliot:<pw> GLOBAL=ethan:<pw> \
//   INSIDE_FILE=/home/amux/wexus/docs/matching-rules.md \
//   OUTSIDE_FILE=/home/amux/mtp/index.html OUT_DIR=/tmp/shots \
//   node cloud/desktop/scope-verify.mjs
//
// It creates one card on an in-group worker and deletes it; nothing else is
// written. Run from the repo root.
import { createRequire } from 'node:module';
import fs from 'node:fs';
const require = createRequire(new URL('../../e2e/package.json', import.meta.url));
const { chromium } = require('@playwright/test');

const BASE = process.env.AMUX_REMOTE;
const GROUP = process.env.GROUP;
const [SU, SP] = (process.env.SCOPED || '').split(/:(.*)/s);
const [GU, GP] = (process.env.GLOBAL || '').split(/:(.*)/s);
const INSIDE = process.env.INSIDE_FILE;
const OUTSIDE = process.env.OUTSIDE_FILE;
const OUT = process.env.OUT_DIR || '.';
if (!BASE || !GROUP || !SU || !SP || !GU || !GP) { console.error('set AMUX_REMOTE GROUP SCOPED GLOBAL'); process.exit(2); }
fs.mkdirSync(OUT, { recursive: true });

let fails = 0;
const check = (ok, what, detail = '') => { if (!ok) fails++; console.log(`${ok ? 'PASS' : 'FAIL'}  ${what}${detail ? `  (${detail})` : ''}`); };
const hdr = (u, p) => ({ Authorization: 'Basic ' + Buffer.from(`${u}:${p}`).toString('base64'), 'Content-Type': 'application/json' });
const call = (u, p) => (method, path, body) => fetch(BASE + path, { method, headers: hdr(u, p), body: body ? JSON.stringify(body) : undefined });
const asScoped = call(SU, SP), asGlobal = call(GU, GP);
const json = async r => { try { return await r.json(); } catch { return null; } };

// Ground truth from the global account: which workers are in the group.
const all = await json(await asGlobal('GET', '/api/sessions')) || [];
const groupOf = s => (s.tags || s.groups || []).map(String);
const inGroup = all.filter(s => groupOf(s).includes(GROUP)).map(s => s.name).sort();
const outGroup = all.filter(s => !groupOf(s).includes(GROUP)).map(s => s.name).sort();
check(inGroup.length > 0 && outGroup.length > 0, `control: ${GU} sees workers in and out of '${GROUP}'`, `in=${inGroup.join(',')} out=${outGroup.join(',')}`);

// --- API, as the scoped account ---
const ident = await json(await asScoped('GET', '/api/identity')) || {};
check(ident.access_scope?.level === 'group' && ident.access_scope?.name === GROUP, `${SU} is scoped to group '${GROUP}'`, JSON.stringify(ident.access_scope));

const mine = ((await json(await asScoped('GET', '/api/sessions'))) || []).map(s => s.name).sort();
check(JSON.stringify(mine) === JSON.stringify(inGroup), `${SU} sees exactly the '${GROUP}' workers`, `sees=${mine.join(',')}`);

const outsider = outGroup[0], insider = inGroup[0];
let r = await asScoped('GET', `/api/sessions/${outsider}/peek?lines=5`);
check(r.status === 403, `${SU} cannot open a worker outside the group (${outsider})`, `HTTP ${r.status}`);
// Empty text: if the guard ever leaks, the handler rejects it and nothing is
// delivered to a real worker (a non-empty probe once reached one).
r = await asScoped('POST', `/api/sessions/${outsider}/send`, { text: '' });
check(r.status === 403, `${SU} cannot message a worker outside the group`, `HTTP ${r.status}`);
r = await asScoped('GET', `/api/sessions/${insider}/peek?lines=5`);
check(r.status === 200, `${SU} can open an in-group worker (${insider})`, `HTTP ${r.status}`);

const board = (await json(await asScoped('GET', '/api/board?all=1'))) || [];
const foreign = board.filter(c => c.session && !inGroup.includes(c.session));
check(Array.isArray(board) && foreign.length === 0, `${SU}'s board holds only in-group cards`, `cards=${board.length} foreign=${foreign.map(c => c.id).join(',')}`);
const globalBoard = (await json(await asGlobal('GET', '/api/board?all=1'))) || [];
const hidden = globalBoard.find(c => c.session && outGroup.includes(c.session));
if (hidden) {
  r = await asScoped('GET', `/api/board/${hidden.id}`);
  check(r.status === 403, `${SU} cannot open an out-of-group card by id (${hidden.id})`, `HTTP ${r.status}`);
}
r = await asScoped('POST', '/api/board', { title: 'scope check: out of group', session: outsider, status: 'backlog' });
check(r.status >= 400, `${SU} cannot create a card for an out-of-group worker`, `HTTP ${r.status}`);
r = await asScoped('POST', '/api/board', { title: `scope check: in group ${Date.now()}`, session: insider, status: 'backlog' });
const made = await json(r);
check(r.ok && made?.id, `${SU} can create a card for an in-group worker`, `HTTP ${r.status} id=${made?.id} creator=${made?.creator}`);
if (made?.id) await asGlobal('DELETE', `/api/board/${made.id}`);

if (INSIDE) {
  r = await asScoped('GET', '/api/file/raw?path=' + encodeURIComponent(INSIDE));
  check(r.status === 200, `${SU} can read a file in the group's folder`, `${INSIDE} HTTP ${r.status}`);
  r = await asScoped('GET', '/api/file/raw-path' + INSIDE.split('/').map(encodeURIComponent).join('/'));
  check(r.status === 200, `${SU} can open it through the preview URL too`, `HTTP ${r.status}`);
  const dir = INSIDE.replace(/[^/]*$/, '');
  r = await asScoped('GET', '/api/file/raw?path=' + encodeURIComponent(dir + '../../../etc/hostname'));
  check(r.status === 403, `${SU} cannot climb out with '..'`, `HTTP ${r.status}`);
}
if (OUTSIDE) {
  r = await asScoped('GET', '/api/file/raw?path=' + encodeURIComponent(OUTSIDE));
  check(r.status === 403, `${SU} cannot read a file outside the group's folders`, `${OUTSIDE} HTTP ${r.status}`);
  r = await asGlobal('GET', '/api/file/raw?path=' + encodeURIComponent(OUTSIDE));
  check(r.status === 200, `control: ${GU} can read it`, `HTTP ${r.status}`);
}
r = await asScoped('PUT', '/api/scope', { level: 'worker', name: outsider, capability: 'env', value: { X: '1' } });
check(r.status === 403, `${SU} cannot change settings of an out-of-group worker`, `HTTP ${r.status}`);
r = await asScoped('PUT', '/api/scope', { level: 'global', name: '', capability: 'env', value: { X: '1' } });
check(r.status === 403, `${SU} cannot change global settings`, `HTTP ${r.status}`);

// --- Browser, as the scoped account ---
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
const page = await ctx.newPage();
await page.goto(BASE + '/');
await page.fill('input[name="username"]', SU);
await page.fill('input[name="password"]', SP);
await Promise.all([page.waitForURL(u => !new URL(u).pathname.startsWith('/_door/'), { timeout: 30_000, waitUntil: 'commit' }), page.click('button[type="submit"]')]);
await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 30_000 });
// A fresh profile reloads once when the service worker first installs.
await page.waitForEvent('framenavigated', { predicate: f => f === page.mainFrame(), timeout: 12_000 }).catch(() => {});
await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 60_000 });
const wt = page.locator('#wt-overlay.open');
await wt.waitFor({ state: 'visible', timeout: 5_000 }).catch(() => {});
if (await wt.isVisible()) await page.locator('#wt-tooltip .wt-skip').click();
await page.click('#tab-sessions');
// A scoped dashboard boots slower (many config reads are refused): wait for
// worker cards to render rather than sleeping a fixed time.
await page.locator('[data-session]:visible').first().waitFor({ timeout: 20_000 }).catch(() => {});
await page.waitForTimeout(1500);
const shown = (await page.locator('[data-session]:visible').evaluateAll(es => [...new Set(es.map(e => e.dataset.session))])).sort();
check(inGroup.every(n => shown.includes(n)) && !shown.some(n => outGroup.includes(n)),
  `${SU}'s Workers view shows only the '${GROUP}' workers`, `shown=${shown.join(',')}`);
await page.screenshot({ path: `${OUT}/scoped-workers.png`, timeout: 60_000 }).catch(() => {});
if (INSIDE) {
  await page.waitForFunction(p => { if (typeof window.openFilePreview !== 'function') return false; window.openFilePreview(p); return true; }, INSIDE, { timeout: 60_000 });
  await page.waitForTimeout(2500);
  const err = await page.locator('.file-overlay-body').first().innerText().catch(() => '');
  check(!/denied|403|outside/i.test(err) && err.length > 20, `${SU} opens a group document in the file viewer`, err.slice(0, 80).replace(/\n/g, ' '));
  await page.screenshot({ path: `${OUT}/scoped-file.png`, timeout: 60_000 }).catch(() => {});
}
await browser.close();

console.log(`${fails ? 'FAILED' : 'ALL PASSED'}: ${fails} failure(s); screenshots in ${OUT}`);
process.exit(fails ? 1 : 0);

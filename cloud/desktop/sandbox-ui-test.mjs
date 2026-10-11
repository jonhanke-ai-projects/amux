// Functional UI test for a dedicated amux host, run from another machine in a
// real Chromium. Complements browser-verify.mjs (identity, attribution,
// typing presence) with the things a person does day to day:
//
//   1. open an HTML file in the file viewer and click its links
//      (in-page anchors must scroll in place, never land on the sign-in page)
//   2. turn Chat on for a worker (a member is a human: no 403)
//   3. switch environments in Settings > Connect > Connections, from a
//      LOCAL amux to this host and back, signing in with a stored password
//
//   AMUX_REMOTE=https://sandbox.amux.io USER=ethan PASS=... \
//   LOCAL=https://<your-local-amux>:8824 \
//   HTML_FILE=/home/amux/mtp/index.html OUT_DIR=/tmp/shots \
//   node cloud/desktop/sandbox-ui-test.mjs
//
// Step 2 uses a scratch worker (qa-scratch) it declares and archives itself, so
// no real worker's settings change. Run from the repo root.
import { createRequire } from 'node:module';
import fs from 'node:fs';
const require = createRequire(new URL('../../e2e/package.json', import.meta.url));
const { chromium } = require('@playwright/test');

const BASE = process.env.AMUX_REMOTE;
const USER = process.env.USER_NAME || process.env.AMUX_REMOTE_USER;
const PASS = process.env.PASS || process.env.AMUX_REMOTE_PASS;
const LOCAL = (process.env.LOCAL || '').replace(/\/+$/, '');
const HTML_FILE = process.env.HTML_FILE || '';
const OUT = process.env.OUT_DIR || '.';
const SCRATCH = 'qa-scratch';
if (!BASE || !USER || !PASS) { console.error('set AMUX_REMOTE, AMUX_REMOTE_USER, AMUX_REMOTE_PASS'); process.exit(2); }
fs.mkdirSync(OUT, { recursive: true });

let fails = 0;
const check = (ok, what, detail = '') => {
  if (!ok) fails++;
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${what}${detail ? `  (${detail})` : ''}`);
};
const basic = { Authorization: 'Basic ' + Buffer.from(`${USER}:${PASS}`).toString('base64'), 'Content-Type': 'application/json' };
const api = (method, path, body) => fetch(BASE + path, { method, headers: basic, body: body ? JSON.stringify(body) : undefined });

async function signIn(page, base) {
  await page.goto(base + '/');
  if (new URL(page.url()).pathname.startsWith('/_door/login')) {
    await page.fill('input[name="username"]', USER);
    await page.fill('input[name="password"]', PASS);
    await Promise.all([page.waitForURL(u => !new URL(u).pathname.startsWith('/_door/'), { timeout: 30_000, waitUntil: 'commit' }),
      page.click('button[type="submit"]')]);
  }
  await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 30_000 });
  // A fresh profile reloads once when the service worker first installs
  // (~5s after load). Let that happen before interacting, or the page under
  // test is replaced mid-step.
  await page.waitForEvent('framenavigated', { predicate: f => f === page.mainFrame(), timeout: 12_000 }).catch(() => {});
  await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 30_000 });
  const wt = page.locator('#wt-overlay.open');
  await wt.waitFor({ state: 'visible', timeout: 5_000 }).catch(() => {});
  if (await wt.isVisible()) await page.locator('#wt-tooltip .wt-skip').click();
}

const browser = await chromium.launch();

// 1. HTML file preview: anchors stay in the document.
if (HTML_FILE) {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  const page = await ctx.newPage();
  await signIn(page, BASE);
  await page.waitForFunction(() => typeof window.openFilePreview === 'function', null, { timeout: 60_000 });
  await page.evaluate(p => window.openFilePreview(p), HTML_FILE);
  const frameEl = page.locator('.file-html-preview iframe').first();
  await frameEl.waitFor({ state: 'attached', timeout: 20_000 });
  const frame = page.frameLocator('.file-html-preview iframe').first();
  await frame.locator('a[href^="#"]').first().waitFor({ timeout: 30_000 }).catch(() => {});
  const src = await frameEl.getAttribute('src');
  check(!!src && src.includes('/api/file/raw-path/'), 'preview loads the file from its own URL', `src=${src}`);
  const anchors = await frame.locator('a[href^="#"]').evaluateAll(as => as.map(a => a.getAttribute('href')).slice(0, 4));
  check(anchors.length > 0, 'preview has in-page links to click', anchors.join(' '));
  for (const href of anchors) {
    await frame.locator(`a[href="${href}"]`).first().click();
    await page.waitForTimeout(800);
    const signInShown = await frame.locator('form[action="/_door/login"]').count();
    const stillDoc = await frame.locator(`[id="${href.slice(1)}"], a[name="${href.slice(1)}"]`).count();
    check(signInShown === 0 && stillDoc > 0, `clicking ${href} stays in the document`, `sign-in forms=${signInShown}, target present=${stillDoc}`);
  }
  await page.screenshot({ path: `${OUT}/preview-after-links.png` });
  await ctx.close();
}

// 2. Chat toggle on a scratch worker, as a member (a human).
if (process.env.SKIP_CHAT !== '1') {
  const decl = await api('POST', '/api/env/apply', { workers: [{ name: SCRATCH, dir: '/home/amux', provider: 'claude', desc: 'UI test scratch (safe to archive)' }] });
  check(decl.ok, 'declare scratch worker', `HTTP ${decl.status}`);
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  const page = await ctx.newPage();
  await signIn(page, BASE);
  const toasts = [];
  page.on('console', () => {});
  // Call each function inside the wait that sees it: the dashboard can reload
  // itself once after a version change, between a wait and a later evaluate.
  await page.waitForFunction(n => { if (typeof window.openPeek !== 'function') return false; window.openPeek(n); return true; }, SCRATCH, { timeout: 60_000 });
  await page.waitForTimeout(1500);
  await page.waitForFunction(() => { if (typeof window._peekChatSet !== 'function' || typeof peekSession === 'undefined' || !peekSession) return false; window._peekChatSet(true); return true; }, null, { timeout: 60_000 });
  await page.waitForTimeout(2500);
  const toast = await page.locator('.toast, #toast, [class*="toast"]').allInnerTexts().catch(() => []);
  toasts.push(...toast);
  const sess = await (await api('GET', '/api/sessions')).json().catch(() => []);
  const s = (Array.isArray(sess) ? sess : []).find(x => x.name === SCRATCH) || {};
  check(!toasts.some(t => /403|may not configure/.test(t)), 'turning Chat on shows no permission error', toasts.join(' | ').slice(0, 200));
  check(!!s.chat_companion, 'Chat is on for the scratch worker', `chat_companion=${s.chat_companion}`);
  await page.screenshot({ path: `${OUT}/chat-on.png` });
  await page.evaluate(() => window._peekChatSet && window._peekChatSet(false)).catch(() => {});
  // Archiving is human-only and carries the dashboard's UI token, so it goes
  // through the dashboard's own archiveSession(), as a person's click would.
  await page.waitForFunction(n => { if (typeof window.archiveSession !== 'function') return false; window.archiveSession(n); return true; }, SCRATCH, { timeout: 30_000 });
  await page.waitForTimeout(3000);
  await ctx.close();
  const row = ((await (await api('GET', '/api/sessions')).json().catch(() => [])) || []).find(x => x.name === SCRATCH);
  check(!row || row.archived === true, 'scratch worker archived from the dashboard', row ? `archived=${row.archived}` : 'gone');
}

// 3. Environment switching: LOCAL -> this host (stored password) -> LOCAL.
if (LOCAL) {
  const ctx = await browser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1280, height: 860 } });
  // A non-loopback browser needs the local server's owner token. Attach it to
  // requests for the LOCAL origin only, so it never travels to the remote.
  if (process.env.LOCAL_TOKEN) {
    await ctx.route(u => u.origin === LOCAL, route =>
      route.continue({ headers: { ...route.request().headers(), authorization: 'Bearer ' + process.env.LOCAL_TOKEN } }));
  }
  const page = await ctx.newPage();
  await page.goto(LOCAL + '/');
  await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 60_000 }).catch(() => {});
  // Let the one-time cache-clear / service-worker reload happen first.
  await page.waitForEvent('framenavigated', { predicate: f => f === page.mainFrame(), timeout: 12_000 }).catch(() => {});
  await page.waitForFunction(() => typeof window._addConnectionSave === 'function', null, { timeout: 60_000 }).catch(() => {});
  const localOk = await page.evaluate(() => typeof window._addConnectionSave === 'function').catch(() => false);
  check(localOk, 'local amux dashboard loads', page.url());
  if (localOk) {
    // toggleSettings() TOGGLES: open the panel only if the form is not already reachable.
    await page.evaluate(() => { _settingsTab('integrations'); _toggleAddConnectionForm(true); });
    if (!(await page.locator('#add-conn-url').isVisible())) {
      await page.evaluate(() => { toggleSettings(); _settingsTab('integrations'); _toggleAddConnectionForm(true); });
    }
    await page.locator('#add-conn-url').waitFor({ state: 'visible', timeout: 15_000 });
    await page.fill('#add-conn-name', 'Sandbox');
    await page.fill('#add-conn-url', BASE);
    await page.fill('#add-conn-user', USER);
    await page.fill('#add-conn-pass', PASS);
    await page.locator('#add-conn-form button', { hasText: 'Add' }).click();
    const row = page.locator('#settings-connections-list > div', { hasText: new URL(BASE).host }).first();
    await Promise.all([page.waitForURL(u => u.origin === new URL(BASE).origin && !u.pathname.startsWith('/_door/'), { timeout: 40_000, waitUntil: 'commit' }),
      row.locator('a', { hasText: 'Open' }).click()]);
    await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 30_000 });
    const ident = await page.evaluate(async () => (await fetch('/api/identity')).json());
    check(ident.is_local_member && ident.email, `switched local -> ${new URL(BASE).host}, signed in`, `email=${ident.email}`);
    const conns = await page.evaluate(() => JSON.parse(localStorage.getItem('amux_connections') || '[]'));
    const back = conns.findIndex(c => c.url.replace(/\/+$/, '') === LOCAL);
    check(back >= 0 && !JSON.stringify(conns).includes(PASS), 'the remote received the connection list (without the password)', `entries=${conns.length}`);
    await page.screenshot({ path: `${OUT}/switched-to-remote.png` });
    if (back >= 0) {
      await Promise.all([page.waitForURL(u => u.origin === LOCAL, { timeout: 40_000, waitUntil: 'commit' }),
        page.evaluate(i => window._switchServerUrl(i), back)]);
      await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 60_000 }).catch(() => {});
      check(new URL(page.url()).origin === LOCAL, `switched back ${new URL(BASE).host} -> local`, page.url().split('?')[0]);
      await page.screenshot({ path: `${OUT}/switched-back-local.png` });
    }
  }
  await ctx.close();
}

await browser.close();
console.log(`${fails ? 'FAILED' : 'ALL PASSED'}: ${fails} failure(s); screenshots in ${OUT}`);
process.exit(fails ? 1 : 0);

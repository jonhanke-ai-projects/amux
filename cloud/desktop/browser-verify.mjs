// Browser acceptance test for a dedicated amux host (cloud/desktop/bootstrap.sh).
//
// For each user: a fresh browser context signed in with that user's basic-auth
// credentials loads the real dashboard, creates a board card through the board
// UI, and sends a message to a worker through the peek composer. Afterwards it
// reads the board back over the API and checks that each UI action is recorded
// under the user who made it. Screenshots go to $OUT_DIR.
//
//   AMUX_REMOTE=https://wexus.amux.io \
//   USERS='ethan:pw elliot:pw doron:pw' WORKER=ed-payment-matcher \
//   OUT_DIR=/tmp/shots node cloud/desktop/browser-verify.mjs
//
// Run from the repo root so e2e/node_modules resolves playwright.
import { createRequire } from 'node:module';
import fs from 'node:fs';
const require = createRequire(new URL('../../e2e/package.json', import.meta.url));
const { chromium } = require('@playwright/test');

const BASE = process.env.AMUX_REMOTE;
const WORKER = process.env.WORKER || 'ed-payment-matcher';
const OUT = process.env.OUT_DIR || '.';
const users = (process.env.USERS || '').split(/\s+/).filter(Boolean).map(p => {
  const i = p.indexOf(':'); return { user: p.slice(0, i), pass: p.slice(i + 1) };
});
if (!BASE || !users.length) { console.error('set AMUX_REMOTE and USERS'); process.exit(2); }
fs.mkdirSync(OUT, { recursive: true });


// Sign in the way a person does: the host's amux-door form (Basic credentials
// only reach a browser after a challenge, and the door redirects instead).
async function signIn(page, user, pass) {
  await page.goto(BASE + '/');
  if (new URL(page.url()).pathname.startsWith('/_door/login')) {
    await page.fill('input[name="username"]', user);
    await page.fill('input[name="password"]', pass);
    await Promise.all([page.waitForURL(u => !new URL(u).pathname.startsWith('/_door/'), { timeout: 30_000, waitUntil: 'commit' }), page.click('button[type="submit"]')]);
  }
  // A fresh profile reloads once when the service worker first installs;
  // interacting before that lands on a page about to be replaced.
  await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 60_000 }).catch(() => {});
  await page.waitForEvent('framenavigated', { predicate: f => f === page.mainFrame(), timeout: 12_000 }).catch(() => {});
  await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 60_000 }).catch(() => {});
  return page;
}

let fails = 0;
const check = (ok, what, detail = '') => {
  if (!ok) fails++;
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${what}${detail ? `  (${detail})` : ''}`);
};

const browser = await chromium.launch();

// Remove cards a previous run left behind (a crash skips the cleanup at the
// end), or board intake folds this run's creates into them as appends.
const TOPICS = ['Confirm the July Coeur invoices reconcile to zero', 'Chase the bounced Leon & Son payment from last week',
                'Check the summary tab partner rollup formatting', 'Audit orphan closures against credit memos'];
{
  const a = { Authorization: 'Basic ' + Buffer.from(`${users[0].user}:${users[0].pass}`).toString('base64') };
  const all = await (await fetch(BASE + '/api/board?all=1', { headers: a })).json().catch(() => []);
  for (const c of Array.isArray(all) ? all : []) {
    if (TOPICS.some(t => (c.title || '').startsWith(t))) await fetch(BASE + `/api/board/${c.id}`, { method: 'DELETE', headers: a });
  }
}
const stamp = Date.now().toString(36);
const made = [];

// No credentials at all: the browser must be refused.
{
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await page.goto(BASE + '/');
  const refused = new URL(page.url()).pathname.startsWith('/_door/login');
  const api = await page.evaluate(async () => (await fetch('/api/sessions')).status);
  check(refused && api === 401, 'browser without credentials gets the sign-in page and API 401s', `at ${page.url()}, api ${api}`);
  // A wrong password stays on the sign-in page.
  await page.fill('input[name="username"]', users[0].user);
  await page.fill('input[name="password"]', 'definitely-wrong');
  await page.click('button[type="submit"]');
  await page.waitForLoadState();
  check(/Wrong username or password/.test(await page.content()), 'wrong password is refused on the form');
  await ctx.close();
}

for (const { user, pass } of users) {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  const page = await ctx.newPage();
  await signIn(page, user, pass);
  const onDash = !new URL(page.url()).pathname.startsWith('/_door/');
  check(onDash, `${user}: signs in through the form and reaches the dashboard`, page.url());
  await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 30_000 });
  // Same first-visit obstacles the e2e suite dismisses, the way a user would.
  await page.addLocatorHandler(page.locator('#sw-fail-bar'), async bar => { await bar.locator('button').last().click(); });
  const wt = page.locator('#wt-overlay.open');
  await wt.waitFor({ state: 'visible', timeout: 8_000 }).catch(() => {});
  if (await wt.isVisible()) await page.locator('#wt-tooltip .wt-skip').click();
  await page.screenshot({ path: `${OUT}/${user}-1-dashboard.png` });
  const ident = await page.evaluate(async () => (await fetch('/api/identity')).json());
  const actor = ident.is_local_member && ident.email ? `member:${ident.email}` : '';
  check(!!actor, `${user}: browser session is a verified amux member`, `email=${ident.email} scope=${JSON.stringify(ident.access_scope)}`);

  // 1. Create a card through the board UI.
  // Distinct work per user: board intake folds look-alike titles into one card
  // (AMUX-4776), which would hide whose card is whose.
  const title = `${TOPICS[made.length % TOPICS.length]} (${user}, ${stamp})`;
  await page.click('#tab-board');
  await page.locator('#board-view').waitFor({ state: 'visible' });
  await page.locator('.board-new-btn:visible').first().click();
  await page.fill('#be-title', title);
  const saved = page.waitForResponse(r => r.url().endsWith('/api/board') && r.request().method() === 'POST', { timeout: 15_000 });
  await page.locator('.be-save:visible').first().click();
  const sr = await saved.catch(() => null);
  const sb = sr ? await sr.json().catch(() => ({})) : {};
  // A group- or worker-scoped account may only create cards for its own
  // workers, so the board's unowned "+ New issue" card is correctly refused.
  const scoped = ident.access_scope && ident.access_scope.level !== 'global';
  if (scoped) {
    check(!!sr && sr.status() === 403, `${user}: scoped account cannot create an unowned card`, sr ? `HTTP ${sr.status()}` : 'no save request');
  } else {
    check(!!sr && sr.status() === 201 && sb.card_created === true, `${user}: card created through the board UI`,
      sr ? `HTTP ${sr.status()} id=${sb.id} creator=${sb.creator}` : 'no save request');
  }
  await page.screenshot({ path: `${OUT}/${user}-2-board.png` });

  // 2. Message a worker (opt-in: on a shared host this writes into a worker
  // someone may be using; SEND_MESSAGES=1 enables it).
  const msg = `Browser check from ${user} (${stamp}): reply with one line naming the file you would read first. Do not change anything.`;
  if (process.env.SEND_MESSAGES === '1') {
  // 2. Message a worker through the peek composer.
    await page.click('#tab-sessions');
    const card = page.locator(`[data-session="${WORKER}"]:visible`).first();
    await card.click({ position: { x: 120, y: 22 } });   // the card header, where the name is
    const input = card.locator('textarea:visible').first();
    await input.waitFor({ state: 'visible', timeout: 15_000 });
    await input.fill(msg);
    const sent = page.waitForResponse(r => r.url().includes(`/api/sessions/${WORKER}/send`) && r.request().method() === 'POST', { timeout: 15_000 });
    await card.getByRole('button', { name: 'Send', exact: true }).first().click();
    const mr = await sent.catch(() => null);
    check(!!mr && mr.ok(), `${user}: message sent to ${WORKER} from the worker card`, mr ? `HTTP ${mr.status()}` : 'no send request');
    await page.waitForTimeout(4000);
    await page.screenshot({ path: `${OUT}/${user}-3-message.png` });
  }
  made.push({ user, actor, title, scoped, msg: process.env.SEND_MESSAGES === '1' ? msg : '' });
  await ctx.close();
}
// 3. Multiplayer typing presence (AC-474): the second user types into a
// worker's composer without sending; the first user, on the same worker card,
// must see "<label> is typing…", and it must clear once the draft is emptied.
if (users.length >= 2) {
  const open = async ({ user, pass }) => {
    const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
    const page = await ctx.newPage();
    await signIn(page, user, pass);
    await page.waitForFunction(() => typeof window.apiCall === 'function', null, { timeout: 30_000 });
    await page.addLocatorHandler(page.locator('#sw-fail-bar'), async bar => { await bar.locator('button').last().click(); });
    const wt = page.locator('#wt-overlay.open');
    await wt.waitFor({ state: 'visible', timeout: 8_000 }).catch(() => {});
    if (await wt.isVisible()) await page.locator('#wt-tooltip .wt-skip').click();
    await page.click('#tab-sessions');
    const card = page.locator(`[data-session="${WORKER}"]:visible`).first();
    await card.click({ position: { x: 120, y: 22 } });
    const input = card.locator('textarea:visible').first();
    await input.waitFor({ state: 'visible', timeout: 15_000 });
    return { ctx, page, card, input };
  };
  const watcher = await open(users[0]);
  const typist = await open(users[1]);
  const typistLabel = users[1].user;
  await typist.input.fill('');
  await typist.input.pressSequentially('checking the July closures', { delay: 40 });
  const ind = watcher.page.locator(`#typing-card-${WORKER}`);
  const seen = await ind.waitFor({ state: 'visible', timeout: 10_000 }).then(() => ind.innerText(), () => '');
  await watcher.page.screenshot({ path: `${OUT}/typing-${users[0].user}-sees.png` });
  check(/is typing/.test(seen), `${users[0].user} sees ${typistLabel} typing to ${WORKER}`, `indicator=${JSON.stringify(seen)}`);
  const ib = await ind.boundingBox().catch(() => null);
  const inb = await watcher.input.boundingBox().catch(() => null);
  check(!!ib && !!inb && ib.y + ib.height <= inb.y + 1, 'typing indicator sits above the composer input',
    ib && inb ? `indicator bottom=${Math.round(ib.y + ib.height)} input top=${Math.round(inb.y)}` : 'no boxes');
  const selfInd = await typist.page.locator(`#typing-card-${WORKER}`).count();
  check(selfInd === 0, `${typistLabel} does not see their own typing indicator`, `elements=${selfInd}`);
  await typist.input.fill('');
  await typist.input.dispatchEvent('input');
  const cleared = await ind.waitFor({ state: 'detached', timeout: 12_000 }).then(() => true, () => false);
  check(cleared, `indicator clears when ${typistLabel} empties the draft`);
  await watcher.ctx.close(); await typist.ctx.close();
}
await browser.close();

// 3. Read back over the API (as the first user) and check attribution.
const [{ user: u0, pass: p0 }] = users;
const auth = { Authorization: 'Basic ' + Buffer.from(`${u0}:${p0}`).toString('base64') };
const board = await (await fetch(BASE + '/api/board?all=1&slim=0', { headers: auth })).json();
for (const m of made.filter(x => !x.scoped)) {
  const card = board.find(c => c.title === m.title);
  const creator = card?.creator || '';
  check(!!card && !!m.actor && creator === m.actor,
    `${m.user}: UI card attributed to ${m.actor || m.user}`, `creator=${JSON.stringify(creator)} id=${card?.id}`);
}
const hist = await (await fetch(BASE + `/api/history?session=${encodeURIComponent(WORKER)}&limit=50`, { headers: auth })).json().catch(() => null);
const rows = Array.isArray(hist) ? hist : (hist?.items || hist?.history || []);
for (const m of made.filter(x => x.msg)) {
  const row = rows.find(h => (h.text || '').includes(m.msg.slice(0, 40)));
  const email = m.actor.replace(/^member:/, '');
  const who = row ? Object.entries(row).filter(([k, v]) => typeof v === 'string' && email && v.includes(email)).map(([k, v]) => `${k}=${v}`) : [];
  check(!!row && who.length > 0, `${m.user}: UI message to ${WORKER} attributed to ${m.actor || m.user}`,
    row ? (who.length ? who.join(', ') : `row has no ${email}: ${JSON.stringify(row).slice(0, 200)}`) : 'message not found in history');
}
// Remove this run's cards: board intake folds a later run's look-alike titles
// into leftovers, which turns the next run's creates into appends (200, not 201).
for (const m of made) {
  const card = board.find(c => c.title === m.title);
  if (card) await fetch(BASE + `/api/board/${card.id}`, { method: 'DELETE', headers: auth });
}
fs.writeFileSync(`${OUT}/made.json`, JSON.stringify({ stamp, made: made.map(m => ({ user: m.user, title: m.title })) }, null, 2));
console.log(`${fails ? 'FAILED' : 'ALL PASSED'}: ${fails} failure(s); screenshots in ${OUT}`);
process.exit(fails ? 1 : 0);

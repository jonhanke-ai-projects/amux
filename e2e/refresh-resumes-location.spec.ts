// Ethan, 2026-10-22: "when i refresh a page it should resume that
// state/location". A page opened from a deep link kept that link in its URL
// while you moved on, and a refresh went back to the link; every Mac app window
// opens with one. The URL now follows the location.
import {test, expect, Page, allowUnusedRoute} from './fixtures';

async function routes(page: Page) {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.route(/\/api\/sessions(?:\?.*)?$/, r => r.fulfill({json: [{name: 'refw', running: true, status: 'idle', dir: '/tmp'}]}));
  await page.route(/\/api\/sessions\/refw\/peek\?/, r => r.fulfill({json: {name: 'refw', live: 'ready', history: ''}}));
  await page.route(/\/api\/sessions\/refw\/subagents$/, r => r.fulfill({json: {session: 'refw', subagents: []}}));
  // A view-only case never opens the worker.
  allowUnusedRoute(page, /\/api\/sessions\/refw\/peek\?/);
  allowUnusedRoute(page, /\/api\/sessions\/refw\/subagents$/);
}
const ready = "typeof switchView === 'function' && typeof openPeek === 'function' && Array.isArray(sessions) && sessions.some(s => s.name === 'refw')";
const where = (page: Page) => page.evaluate(`({ view: activeView, peek: document.getElementById('peek-overlay').classList.contains('active') ? peekSession + ':' + _peekTab : null, hash: location.hash })`) as Promise<any>;
async function load(page: Page, url: string) {
  await page.goto(url, {waitUntil: 'domcontentloaded'});
  await page.waitForFunction(ready, null, {timeout: 30000});
  await page.waitForTimeout(2600);   // the boot link and saved screen apply, then the URL follows
}
async function refresh(page: Page) {
  await page.reload({waitUntil: 'domcontentloaded'});
  await page.waitForFunction(ready, null, {timeout: 30000});
  await page.waitForTimeout(2000);
}

test('a page opened from #view= refreshes to the view you moved to', async ({page}) => {
  await routes(page);
  await load(page, '/#view=board');
  await page.evaluate("switchView('messages')");
  await expect.poll(() => where(page).then(w => w.hash)).toBe('#view=messages');
  await refresh(page);
  expect((await where(page)).view).toBe('messages');
});

test('a worker opened from #peek= refreshes to the tab and view you moved to', async ({page}) => {
  await routes(page);
  await load(page, '/#peek=refw&tab=chat');
  await page.evaluate("setPeekTab('messages')");
  await expect.poll(() => where(page).then(w => w.hash)).toMatch(/^#peek=refw&tab=messages&view=/);
  const before = await where(page);
  await refresh(page);
  await expect.poll(() => where(page).then(w => w.peek), {timeout: 10000}).toBe('refw:messages');
  expect((await where(page)).view).toBe(before.view);
});

test('closing the worker and moving on is what a refresh resumes, not the link', async ({page}) => {
  await routes(page);
  await load(page, '/#peek=refw&tab=chat');
  await page.evaluate("closePeek(); switchView('calendar')");
  await expect.poll(() => where(page).then(w => w.hash)).toBe('#view=calendar');
  await refresh(page);
  const w = await where(page);
  expect(w.view).toBe('calendar');
  expect(w.peek).toBeNull();
});

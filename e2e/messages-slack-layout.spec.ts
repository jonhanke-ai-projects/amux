// Global Messages page (Ethan, 2026-10-11):
// - the composer stays at the bottom without scrolling the page; the posts and
//   the sidebar each scroll on their own, like Slack;
// - clicking a worker shows its messages (its history, not only posts made here);
// - a worker name in All messages opens that worker and inserts nothing;
// - the composer does what worker chat does: offline sends queue visibly, and
//   attachments upload and ride the post.
// The conversation list, posts and history are stubbed (the test server has no
// workers); uploads use the real server.
import { test, expect, Page, allowUnusedRoute } from './fixtures';

const ME = { email: 'owner', key: 'person:owner', kind: 'person', name: 'owner', scope_level: 'global', scope_name: '' };
const WORKERS = Array.from({ length: 40 }, (_, i) => ({ conv: 'w:w' + i, name: 'w' + i, online: false, unread: 0, mentions: 0, last_ts: 0 }));
const LIST = { measured: true, n_considered: 41, me: ME,
  channels: [{ conv: 'g:general', name: 'general', unread: 0, mentions: 0, last_ts: 0 }],
  people: [{ conv: 'p:owner|owner', email: 'owner', name: 'owner', me: true, online: true, unread: 0, mentions: 0 }],
  workers: WORKERS };
const now = Date.now();
const POSTS = Array.from({ length: 80 }, (_, i) => ({ id: i + 1, conv: 'g:general', author_key: 'person:owner', author_name: 'owner',
  body: 'post number ' + (i + 1), mentions: [], deliveries: [], ts: now - (80 - i) * 60000 }));

async function setup(page: Page, opts: { postFails?: boolean } = {}) {
  const posted: string[] = [];
  await page.route(/\/api\/conversations(\?.*)?$/, r => r.fulfill({ json: LIST }));
  await page.route(/\/api\/conversations\/[^/]+\/posts(\?.*)?$/, async r => {
    const conv = decodeURIComponent(r.request().url().split('/api/conversations/')[1].split('/')[0]);
    if (r.request().method() === 'POST') {
      posted.push(r.request().postDataJSON().body);
      if (opts.postFails) return r.abort('internetdisconnected');
      return r.fulfill({ json: { ok: true, id: 999 } });
    }
    const posts = conv === 'g:general' ? POSTS : [];
    return r.fulfill({ json: { measured: true, n_considered: posts.length, posts, people: [ME], workers: [], mentionable_workers: ['w1'], typing: [] } });
  });
  await page.route(/\/api\/conversations\/[^/]+\/(read|typing)$/, r => r.fulfill({ json: { ok: true } }));
  // The All-messages test never opens a conversation, so these two go unused there.
  allowUnusedRoute(page, /\/api\/conversations\/[^/]+\/posts(\?.*)?$/);
  allowUnusedRoute(page, /\/api\/conversations\/[^/]+\/(read|typing)$/);
  await page.addInitScript(() => { localStorage.setItem('amux_walkthrough_done', '1'); localStorage.removeItem('amux_cv_sel'); });
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any)._cvSelect === 'function');
  await page.evaluate(() => (window as any).switchView('messages'));
  return posted;
}

test('composer stays on screen; posts and sidebar scroll on their own', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => (0, eval)("_cvSelect('g:general')"));
  await expect(page.locator('.cv-post').first()).toBeVisible();
  const vh = await page.evaluate(() => window.innerHeight);
  const comp = await page.locator('#cv-compose').boundingBox();
  expect(comp, 'composer rendered').not.toBeNull();
  expect(comp!.y + comp!.height, 'composer bottom is inside the viewport without scrolling').toBeLessThanOrEqual(vh + 1);
  expect(await page.evaluate(() => document.scrollingElement!.scrollTop)).toBe(0);
  const posts = await page.evaluate(() => { const b = document.getElementById('cv-posts')!; return { sh: b.scrollHeight, ch: b.clientHeight, st: b.scrollTop }; });
  expect(posts.sh, 'posts overflow their box').toBeGreaterThan(posts.ch);
  expect(posts.st, 'opens at the newest post').toBeGreaterThan(0);
  if (!(await page.evaluate(() => matchMedia('(max-width: 600px)').matches))) {
    const side = await page.evaluate(() => { const s = document.getElementById('cv-side')!; s.scrollTop = 99999; return { sh: s.scrollHeight, ch: s.clientHeight, st: s.scrollTop }; });
    expect(side.sh, 'sidebar overflows').toBeGreaterThan(side.ch);
    expect(side.st, 'sidebar scrolls').toBeGreaterThan(0);
    expect(await page.evaluate(() => document.scrollingElement!.scrollTop), 'the page itself did not scroll').toBe(0);
  }
});

test('a worker thread shows that worker\'s message history', async ({ page }) => {
  await page.route(/\/api\/history\?/, r => {
    const u = r.request().url();
    if (!u.includes('session=w1')) return r.continue();
    return r.fulfill({ json: [
      { id: 7, session: 'w1', type: 'user', origin: '', text: '[09:00 AM] fix the login page', ts: now - 3600000 },
      { id: 8, session: 'w1', type: 'user', origin: '', text: '[g:x] owner: hi\n\n(From amux Messages. To reply in #x, post plain text: curl ...)', ts: now - 1800000 },
      { id: 9, session: 'w1', type: 'user', origin: '', text: "owner mentioned you in #x (amux Messages):\n\nping\n\nThis is addressed to you: act on it now. Post it with:\namux post g:x <<'EOF'\n<your answer>\nEOF", ts: now - 900000 },
    ] });
  });
  await setup(page);
  await page.evaluate(() => (0, eval)("_cvSelect('w:w1')"));
  await expect(page.locator('#cv-posts')).toContainText('fix the login page');
  await expect(page.locator('#cv-posts')).toContainText('sent from its page');
  await expect(page.locator('#cv-posts'), 'a Messages delivery is not shown twice').not.toContainText('From amux Messages');
  await expect(page.locator('#cv-posts'), 'nor in the new delivery format').not.toContainText('mentioned you in #x');
});

test('offline: a post queues visibly instead of vanishing', async ({ page }) => {
  const posted = await setup(page, { postFails: true });
  await page.evaluate(() => (0, eval)("_cvSelect('g:general')"));
  await page.locator('#cv-input').fill('sent while offline');
  await page.locator('#cv-send').click();
  await expect.poll(() => posted.length).toBeGreaterThan(0);
  await expect(page.locator('.cv-post.pending')).toContainText('sent while offline');
  await expect(page.locator('.cv-post.pending')).toContainText('Queued');
  await expect(page.locator('#cv-input')).toHaveValue('');
});

test('an attachment uploads and rides the post as an @path', async ({ page }) => {
  const posted = await setup(page);
  await page.evaluate(() => (0, eval)("_cvSelect('g:general')"));
  await page.locator('#cv-file').setInputFiles({ name: 'note.txt', mimeType: 'text/plain', buffer: Buffer.from('hello') });
  await expect(page.locator('#card-attach-__messages__ .peek-attach-chip')).toContainText('note.txt');
  await expect.poll(() => page.evaluate(() => ((0, eval)('_cardFiles')['__messages__'] || []).every((f: any) => f.path)), { timeout: 20000 }).toBe(true);
  await page.locator('#cv-input').fill('see attached');
  await page.locator('#cv-send').click();
  await expect.poll(() => posted.length).toBeGreaterThan(0);
  expect(posted[0]).toMatch(/^see attached @\S+note\.txt$/);
  await expect(page.locator('#card-attach-__messages__ .peek-attach-chip')).toHaveCount(0);
});

test('in All messages a worker name opens the worker and inserts nothing', async ({ page }) => {
  await page.route(/\/api\/history\?/, r => r.request().url().includes('counts=1')
    ? r.fulfill({ json: { all: 1, human: 1 } })
    : r.fulfill({ json: [{ id: 5, session: 'w1', type: 'user', kind: 'human', origin: '', text: 'please do the thing', ts: now }] }));
  await setup(page);
  await page.evaluate(() => (0, eval)("_cvSelect('@all')"));
  const link = page.locator('#msgs-log-pane .msg-sess-link').first();
  await expect(link).toHaveText('w1');
  await link.click();
  await page.waitForTimeout(900);
  expect(await page.evaluate(() => (0, eval)('peekSession'))).toBe('w1');
  expect(await page.evaluate(() => (document.getElementById('peek-cmd-input') as HTMLTextAreaElement | null)?.value || ''), 'nothing was inserted').toBe('');
});

// Messages posts render like worker Chat (Ethan, 2026-10-11: "make sure chat
// has the same rendering stuff", screenshot of an `@/Users/.../uploads/x.png`
// post in #amux shown as dead text). A post body goes through the shared
// markdown renderer and _linkifyPaths, so an @upload ref and a plain absolute
// path are both clickable, and an image ref shows a thumbnail.
import { test, expect, Page } from './fixtures';

const ME = { email: 'owner', key: 'person:owner', kind: 'person', name: 'owner', scope_level: 'global', scope_name: '' };
const LIST = { measured: true, n_considered: 1, me: ME,
  channels: [{ conv: 'g:amux', name: 'amux', unread: 0, mentions: 0, last_ts: 0 }],
  people: [{ conv: 'p:owner|owner', email: 'owner', name: 'owner', me: true, online: true, unread: 0, mentions: 0 }],
  workers: [] };
const UPLOAD = '/Users/ethan/.amux/uploads/72900fd7bf3b-image.png';
const PLAIN = '/tmp/amux-e2e/report.csv';
const BODY = '@' + UPLOAD + '\nsee ' + PLAIN + ' and **bold**';

async function setup(page: Page) {
  await page.route(/\/api\/conversations(\?.*)?$/, r => r.fulfill({ json: LIST }));
  await page.route(/\/api\/conversations\/[^/]+\/posts(\?.*)?$/, r => r.fulfill({ json: {
    measured: true, n_considered: 1, people: [ME], workers: [], mentionable_workers: [], typing: [],
    posts: [{ id: 1, conv: 'g:amux', author_key: 'person:owner', author_name: 'owner', body: BODY, mentions: [], deliveries: [], ts: Date.now() }] } }));
  await page.route(/\/api\/conversations\/[^/]+\/(read|typing)$/, r => r.fulfill({ json: { ok: true } }));
  // The thumbnail fetch: a 1x1 PNG so the <img> loads without a real upload.
  await page.route(/\/api\/file\/raw\?/, r => r.fulfill({ contentType: 'image/png',
    body: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64') }));
  await page.addInitScript(() => { localStorage.setItem('amux_walkthrough_done', '1'); localStorage.removeItem('amux_cv_sel'); });
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any)._cvSelect === 'function');
  await page.evaluate(() => (window as any).switchView('messages'));
  await page.evaluate(() => (0, eval)("_cvSelect('g:amux')"));
}

test('an @upload ref and an absolute path in a post are links, the image shows a thumbnail', async ({ page }) => {
  await setup(page);
  const body = page.locator('.cv-post-body').first();
  await expect(body).toBeVisible();
  const upload = body.locator('.file-link', { hasText: UPLOAD });
  await expect(upload, 'the @upload ref is a link').toHaveCount(1);
  await expect(upload).toHaveText(UPLOAD);
  await expect(body.locator('.file-link', { hasText: PLAIN }), 'the plain absolute path is a link').toHaveCount(1);
  await expect(body.locator('strong'), 'markdown renders').toHaveText('bold');
  const thumb = body.locator('img.cv-thumb');
  await expect(thumb, 'the image upload shows a preview').toHaveCount(1);
  await expect(thumb).toHaveAttribute('data-path', UPLOAD);
  await expect.poll(() => thumb.evaluate((i: HTMLImageElement) => i.complete && i.naturalWidth > 0)).toBe(true);
  // Clicking the link opens the file viewer on the real path (no "@" prefix).
  await page.evaluate(() => { (window as any).__opened = []; (window as any).openFilePreview = (p: string) => (window as any).__opened.push(p); });
  await upload.click();
  await expect.poll(() => page.evaluate(() => (window as any).__opened)).toEqual([UPLOAD]);
});

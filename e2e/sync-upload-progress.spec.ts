// A queued file upload shows its progress in the sync checklist and the pill
// (Ethan, 2026-10-11: a 100 MB file sat behind a bare spinner, "Syncing 0/1").
// The upload endpoints are routed so the second chunk can be held mid-flight.
import {test, expect} from './fixtures';

test('a queued upload shows percent and MB on its sync row and in the pill', async ({page}) => {
  let release: () => void = () => {};
  const held = new Promise<void>(r => { release = r; });
  let chunks = 0;
  await page.route(/\/api\/sessions(?:\?.*)?$/, r => r.fulfill({json: []}));
  await page.route(/\/api\/upload\//, async r => {
    const url = r.request().url();
    if (url.endsWith('/start')) {
      const body = JSON.parse(r.request().postData() || '{}');
      return r.fulfill({json: {id: 'sync-up-1', chunks: body.chunks}});
    }
    if (url.includes('/finish')) return r.fulfill({json: {path: '/tmp/sync-up/big.m4a', url: '/api/uploads/big.m4a'}});
    const m = url.match(/\/api\/upload\/([^/]+)\/chunk\/(\d+)/);
    if (m) {
      chunks++;
      if (Number(m[2]) >= 1) await held;   // the first chunk lands, the rest wait
      return r.fulfill({json: {ok: true, chunk: Number(m[2])}});
    }
    return r.continue();
  });
  await page.addInitScript(() => { localStorage.setItem('amux_walkthrough_done', '1'); localStorage.removeItem('amux_upload_bps'); });
  await page.goto('/', {waitUntil: 'domcontentloaded'});
  await page.waitForFunction(() => typeof (window as any)._upqAdd === 'function' && typeof (window as any).runSyncBanner === 'function');
  await page.evaluate(async () => {
    const file = new File([new Uint8Array(24 * 1024 * 1024)], 'big.m4a', {type: 'audio/mp4'});
    await (window as any)._upqAdd(file, '/tmp/sync-up', 'file');
    // The guarded entry the pill tap uses: one pass at a time. Calling the
    // inner _runSyncBanner directly raced the app's own pass on the same file.
    (0, eval)('_syncBannerRequested = true');
    (window as any).runSyncBanner();
  });
  const row = page.locator('#sync-banner .sync-item.running').filter({hasText: 'big.m4a'});
  await expect(row.locator('.sync-progress-text')).toContainText(/Uploading \d+%/, {timeout: 20000});
  await expect(row.locator('.sync-progress-text')).toContainText('MB');
  await expect.poll(async () => Number(await row.locator('.sync-progress-bar').getAttribute('aria-valuenow'))).toBeGreaterThan(0);
  await expect.poll(() => page.evaluate(() => (0, eval)('_syncPillText'))).toMatch(/Syncing 0\/1 · \d+%/);
  release();
  await expect(page.locator('#sync-banner .sync-item.done').filter({hasText: 'big.m4a'})).toHaveCount(1, {timeout: 30000});
  await expect(page.locator('#sync-banner .sync-progress')).toHaveCount(0);
  expect(chunks).toBeGreaterThan(1);
});

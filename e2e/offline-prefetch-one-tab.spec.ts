// One tab saves the offline copies for the device (2026-10-23). The copies live
// in IndexedDB, shared by every tab, yet every Mac app tab fetched every
// running worker's scrollback at boot and every 10 min. An automatic pass runs
// only in a shown tab and not within 9 min of another tab's; a manual run goes.
import { test, expect } from './fixtures';

test('an automatic offline save runs once per device, a manual one always runs', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => (globalThis as any).eval('typeof _offlinePrefetch') === 'function');
  const peeks: string[] = [];
  await page.route(/\/api\/sessions\/[^/]+\/peek\?lines=200/, r => { peeks.push(r.request().url()); return r.fulfill({ json: { output: 'x', history: 'x' } }); });
  const g = (code: string) => page.evaluate(c => (globalThis as any).eval(c), code);
  // Another tab ran a pass a minute ago.
  await g(`localStorage.setItem('amux_offline_prefetch_at', String(Date.now() - 60000)); _prefetchRunning = false; sessions = [{name:'zz-pf', running:true, status:'active'}];`);
  await g('_offlinePrefetch(false)');
  expect(peeks.length).toBe(0);
  // Past 9 minutes the shown tab runs it and stamps the time for the others.
  await g(`localStorage.setItem('amux_offline_prefetch_at', String(Date.now() - 10 * 60000))`);
  await g('_offlinePrefetch(false)');
  await expect.poll(() => peeks.length).toBe(1);
  expect(Date.now() - Number(await g(`localStorage.getItem('amux_offline_prefetch_at')`))).toBeLessThan(30000);
  await g('_offlinePrefetch(false)');
  await g('_offlinePrefetch(true)');
  await expect.poll(() => peeks.length).toBe(2);
});

// The localStorage write guard (disk RCA 2026-10-14). WebKit appends every
// localStorage write to a SQLite WAL it does not shrink while the app runs; the
// macOS app's reached 13.7G and an older install's 57.9G because the dashboard
// rewrote ~2.5MB of caches on every poll. Identical values are not rewritten,
// and a cache key is written at most once per 30s, flushed when the page hides.
import { test, expect } from './fixtures';

test('a cache key is written once per 30s, identical writes are skipped, and hiding flushes the latest', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => !!(window as any)._lsWrites);
  const r = await page.evaluate(() => {
    const st = (window as any)._lsWrites;
    const before = { ...st };
    localStorage.setItem('zz_probe_cache', 'v1');          // first write: immediate
    localStorage.setItem('zz_probe_cache', 'v2');          // within 30s: deferred
    localStorage.setItem('zz_probe_cache', 'v3');          // still deferred, latest kept
    const afterDefer = localStorage.getItem('zz_probe_cache');
    localStorage.setItem('zz_probe_plain', 'same');
    localStorage.setItem('zz_probe_plain', 'same');        // identical: skipped
    return { afterDefer, deferred: st.deferred - before.deferred, skipped: st.skipped_identical - before.skipped_identical };
  });
  expect(r.afterDefer, 'a second write within 30s does not hit storage').toBe('v1');
  expect(r.deferred).toBe(1);
  expect(r.skipped).toBeGreaterThanOrEqual(1);
  // Hiding the page flushes the latest deferred value.
  await page.evaluate(() => window.dispatchEvent(new Event('pagehide')));
  expect(await page.evaluate(() => localStorage.getItem('zz_probe_cache'))).toBe('v3');
});

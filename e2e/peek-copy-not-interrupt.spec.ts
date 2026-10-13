// Cmd+C is copy (2026-10-13). In the worker view, Cmd+C or Cmd+X with no
// selection sent Ctrl-C / Ctrl-X to the worker; a Ctrl-C from Ethan's Mac app
// stopped mxp-gs12's 10 background agents at 16:10:44Z. Only the real Ctrl key
// interrupts now, and every interrupt key the page sends says where it came
// from. Keys requests are captured, never delivered.
import { test, expect } from './fixtures';

test('Cmd+C and Cmd+X never send keys; Ctrl+C interrupts and names its source', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  const keys: string[] = [];
  const beacons: any[] = [];
  await page.route(/\/api\/sessions\/ck-w\/keys$/, async (route) => {
    keys.push(JSON.parse(route.request().postData() || '{}').keys);
    await route.fulfill({ json: { ok: true, detail: 'sent' } });
  });
  page.on('request', (r) => {
    if (r.url().endsWith('/api/client-debug') && r.method() === 'POST') {
      try { const b = JSON.parse(r.postData() || '{}'); if (b.kind === 'interrupt-key') beacons.push(b); } catch (e) {}
    }
  });
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function');
  await page.evaluate(() => {
    const g = globalThis as any;
    g.eval('sessions').push({ name: 'ck-w', status: 'active', running: true, renderer: 'terminal' });
    g.openPeek('ck-w');
  });
  await expect(page.locator('#peek-overlay')).toBeVisible();
  // Nothing selected, focus off the composer: the copy and cut case.
  await page.evaluate(() => { (document.activeElement as HTMLElement | null)?.blur(); window.getSelection()?.removeAllRanges(); });

  await page.keyboard.press('Meta+c');
  await page.keyboard.press('Meta+x');
  await page.waitForTimeout(500);
  expect(keys, 'Cmd+C / Cmd+X must not reach the worker').toEqual([]);

  await page.evaluate(() => { (document.activeElement as HTMLElement | null)?.blur(); });
  await page.keyboard.press('Control+c');
  await expect.poll(() => keys).toEqual(['C-c']);
  await expect.poll(() => beacons.map(b => [b.session, b.keys, b.source])).toContainEqual(['ck-w', 'C-c', 'peek-shortcut']);
});

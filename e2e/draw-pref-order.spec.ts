// Draw tab: three quick snap toggles each saved the settings with their own
// POST, and whichever finished last won, so the stored settings could be older
// than what the editor showed (draw-tab.spec.ts failed on CI, 2026-10-12).
// The first save is held back here so it would finish last.
import {test, expect} from './fixtures';

test('snap settings are stored in click order even when an early save is slow', async ({page}, info) => {
  test.skip(info.project.name !== 'desktop', 'request ordering is the same on every device');
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).switchView === 'function');
  await page.evaluate(() => (window as any).switchView('draw'));
  await page.waitForFunction(() => { const d = (0, eval)('_draw'); return !!(d && d.api && d.path && !d.opening); }, null, {timeout: 30_000});
  const sent: string[] = [];
  let held = false;
  await page.route('**/api/prefs', async route => {
    const body = route.request().postDataJSON?.();
    if (route.request().method() !== 'POST' || !body || body.key !== 'draw_settings') return route.continue();
    sent.push(body.value);
    if (!held) { held = true; await new Promise(r => setTimeout(r, 800)); }
    await route.continue();
  });
  const before = await page.evaluate(() => JSON.stringify((0, eval)('_draw').settings));
  await page.evaluate(() => { const t = (0, eval)('_drawToggleSnap'); t('grid'); t('align'); t('connect'); });
  const want = await page.evaluate(() => JSON.stringify((0, eval)('_draw').settings));
  expect(want).not.toBe(before);
  const headers = await page.evaluate(() => ({Authorization: 'Bearer ' + (window as any)._AMUX_AUTH_TOKEN}));
  await expect.poll(async () => (await (await page.request.get('/api/prefs?key=draw_settings', {headers})).json()).value, {timeout: 10_000})
    .toBe(want);
  await page.waitForTimeout(1200);
  expect((await (await page.request.get('/api/prefs?key=draw_settings', {headers})).json()).value, 'the last click wins after every save lands').toBe(want);
  expect(sent.length).toBe(3);
});

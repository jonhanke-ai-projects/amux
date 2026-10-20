// The worker Board tab's one switch (Ethan, 2026-10-20 screenshot): on a phone
// the On/Off and Inherit buttons took most of the row and squeezed the Board
// description into a column a few words wide. The row now wraps: the text keeps
// the full width and the buttons sit below it.
import { test, expect } from './fixtures';

test('the Board switch description keeps the row width on a phone', async ({ page, request }, testInfo) => {
  await page.setViewportSize({ width: 375, height: 812 });
  await page.goto('/');
  const token = await page.evaluate(() => (window as any)._AMUX_AUTH_TOKEN as string);
  const auth = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' };
  const walkthrough = page.locator('#wt-overlay.open');
  await walkthrough.waitFor({ state: 'visible', timeout: 2_000 }).catch(() => {});
  if (await walkthrough.isVisible()) await page.locator('#wt-tooltip .wt-skip').click();

  const name = `boardfit-${testInfo.project.name}-${Date.now()}`;
  const created = await request.post('/api/sessions', { headers: auth, data: { name, dir: '/tmp' } });
  expect(created.status()).toBe(201);
  try {
    await page.reload();
    await page.waitForFunction(n => typeof (window as any).openPeek === 'function'
      && (globalThis as any).eval('typeof sessions') !== 'undefined'
      && (globalThis as any).eval('sessions').some((s: any) => s.name === n), name);
    await page.evaluate(n => (window as any).openPeek(n), name);
    await page.locator('#peek-tab-issues').click();
    const row = page.locator('#peek-board-config .pbc-one');
    await expect(row).toBeVisible();
    const m = await row.evaluate(el => {
      const r = el.getBoundingClientRect();
      const t = el.querySelector('.pbc-help')!.getBoundingClientRect();
      const sw = el.querySelector('.board-one-switch')!.getBoundingClientRect();
      const inh = el.querySelector('.board-one-inherit')!.getBoundingClientRect();
      return { row: r.width, text: t.width, textBottom: t.bottom, swTop: sw.top, swRight: sw.right, inhRight: inh.right, rowRight: r.right, rowLeft: r.left, textLeft: t.left };
    });
    expect(m.text, `description ${m.text}px of a ${m.row}px row`).toBeGreaterThan(m.row * 0.9);
    expect(m.swTop, 'the buttons sit below the description').toBeGreaterThanOrEqual(m.textBottom - 1);
    expect(m.textLeft, 'the description is not clipped on the left').toBeGreaterThanOrEqual(m.rowLeft - 1);
    expect(Math.abs(m.inhRight - m.rowRight), 'the buttons stay inside the row, right-aligned').toBeLessThan(2);
    const sw = page.locator('#peek-board-config .board-one-switch');
    const box = await sw.boundingBox();
    expect(box!.height).toBeGreaterThanOrEqual(44);
  } finally {
    await request.delete(`/api/sessions/${encodeURIComponent(name)}`, { headers: auth }).catch(() => {});
  }
});

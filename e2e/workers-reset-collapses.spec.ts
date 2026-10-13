// Reset on the Workers page also collapses every expanded card (Ethan,
// 2026-10-13: "when I click reset, on worker list page it should also un
// expand all the worker cards"). The Reset pill also shows when the only
// thing to reset is an expanded card.
import { test, expect, allowUnusedRoute } from './fixtures';

const fleet = [
  { name: 'rc-one', status: 'idle', running: true, tags: ['rc-a'], dir: '/tmp/rc-one' },
  { name: 'rc-two', status: 'idle', running: true, tags: ['rc-b'], dir: '/tmp/rc-two' },
];

test('Reset clears the group filter and collapses every card', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  // The fleet is just these two; the live stream would replace it.
  await page.route('**/api/events**', r => r.abort());
  allowUnusedRoute(page, '**/api/events**');
  const sessionsList = /\/api\/sessions(?:\?.*)?$/;
  await page.route(sessionsList, r => r.request().method() === 'GET' ? r.fulfill({ json: fleet }) : r.fallback());
  allowUnusedRoute(page, sessionsList);
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).resetGroupView === 'function');
  await page.evaluate((fleet) => {
    const g = globalThis as any;
    const list = g.eval('sessions');
    for (const w of fleet) if (!list.some((x: any) => x.name === w.name)) list.push({ ...w });
    g.switchView('sessions');
  }, fleet);
  const card = (n: string) => page.locator(`#cards .card[data-session="${n}"]`);
  const reset = page.locator('.tag-reset-btn');
  await expect(card('rc-one')).toBeVisible();
  await expect(reset).toHaveCount(0);

  // An expanded card alone is enough to offer Reset.
  await page.evaluate(() => (globalThis as any).toggle('rc-one'));
  await expect(card('rc-one')).toHaveClass(/expanded/);
  await expect(reset).toBeVisible();
  await reset.click();
  await expect(card('rc-one')).not.toHaveClass(/expanded/);
  await expect(reset).toHaveCount(0);

  // Both cards expanded and a group narrowed: Reset undoes all of it.
  await page.evaluate(() => { const g = globalThis as any; g.toggle('rc-one'); g.toggle('rc-two'); });
  await expect(page.locator('#cards .card.expanded')).toHaveCount(2);
  await page.evaluate(() => (globalThis as any).toggleTagFilter('rc-a'));
  await expect(card('rc-two')).toHaveCount(0);
  await reset.click();
  await expect(card('rc-two')).toBeVisible();
  await expect(page.locator('#cards .card.expanded')).toHaveCount(0);
  expect(await page.evaluate(() => (globalThis as any).eval('expanded.size'))).toBe(0);
  await expect(page.locator('#tile-expand-btn')).not.toHaveClass(/active/);
});

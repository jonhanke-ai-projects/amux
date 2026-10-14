// Switching from a worker in Split view to another worker left the terminal
// panel's divider ratio (inline flex "0.57 1 0%") in place. Outside split the
// wrap is a column, so the ratio sized the HEIGHT and the next worker's
// composer stopped ~57% down the window (Ethan, 2026-10-14; the PWA's
// peek-inset-report read gap_below_composer 382). closePeek now leaves split
// through _peekSplitSetActive, and CSS ignores the ratio whenever split is off.
import { test, expect } from './fixtures';

test('closing a split worker does not shrink the next worker', async ({ page }) => {
  await page.setViewportSize({ width: 1500, height: 1000 });
  await page.addInitScript(() => {
    localStorage.setItem('amux_walkthrough_done', '1');
    localStorage.setItem('peekSplitRatio', '0.57 1 0%');
  });
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function' && (window as any).sessions?.length >= 1);
  const name = await page.evaluate(() => (window as any).sessions[0].name);
  const bottom = () => page.evaluate(() => {
    const panel = document.getElementById('peek-terminal-panel')!;
    return { panel: Math.round(panel.getBoundingClientRect().bottom), inline: panel.style.flex, ih: innerHeight };
  });
  await page.evaluate((n) => (window as any).openPeek(n), name);
  await page.waitForTimeout(800);
  const plain = await bottom();
  await page.evaluate(() => (window as any)._peekSplitOpen('files'));
  await page.waitForTimeout(800);
  expect((await bottom()).inline, 'split applies the saved ratio').toBe('0.57 1 0%');
  await page.evaluate(() => (window as any).closePeek());
  await page.evaluate((n) => (window as any).openPeek(n), name);
  await page.waitForTimeout(800);
  const after = await bottom();
  expect(after.inline, 'close cleared the divider ratio').toBe('');
  expect(after.panel).toBe(plain.panel);

  // The CSS guard alone holds even if some path leaves the ratio behind.
  await page.evaluate(() => { document.getElementById('peek-terminal-panel')!.style.flex = '0.57 1 0%'; });
  expect((await bottom()).panel).toBe(plain.panel);
});

// App zoom on Safari / the macOS app scales the root font, never CSS zoom
// (Ethan, 2026-10-14: in the Mac app at 130% the worker view stopped 725px
// above the window bottom; its own report read overlay bottom 968 of
// innerHeight 1259 = 1259/1.30). CSS zoom on the root breaks position:fixed in
// WebKit; Chromium keeps CSS zoom, which it lays out correctly.
import { test, expect } from './fixtures';

test('at 130% WebKit scales the root font and the worker view reaches the window bottom', async ({ page, browserName }) => {
  await page.addInitScript(() => { localStorage.setItem('amux_walkthrough_done', '1'); localStorage.setItem('amux_zoom', '130'); });
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function');
  const root = await page.evaluate(() => ({ zoom: document.documentElement.style.zoom, font: document.documentElement.style.fontSize,
    rootZoom: getComputedStyle(document.documentElement).getPropertyValue('--root-zoom').trim(), coarse: matchMedia('(pointer: coarse)').matches }));
  if (browserName === 'webkit') {
    expect(root.zoom, 'no CSS zoom on WebKit').toBe('');
    expect(root.font).toBe('20.8px');
    expect(root.rootZoom).toBe('1');
  } else if (!root.coarse) {
    expect(root.zoom, 'Chromium desktop keeps CSS zoom').toBe('1.3');
  }
  await page.evaluate(() => {
    const g = globalThis as any;
    g.eval('sessions').push({ name: 'zw-w', status: 'idle', running: true });
    g.openPeek('zw-w');
  });
  await expect(page.locator('#peek-overlay')).toBeVisible();
  await page.waitForTimeout(600);
  const gap = await page.evaluate(() => {
    const bar = document.querySelector('#peek-overlay .peek-cmd-bar')!.getBoundingClientRect();
    const zoom = parseFloat(document.documentElement.style.zoom || '1');
    return Math.round(innerHeight - bar.bottom * (navigator.userAgent.includes('Chrome') ? 1 : zoom));
  });
  expect(gap, 'no blank band under the composer').toBeLessThanOrEqual(24);
});

// Font-size zoom (617377ec, Apple WebKit) grows text but not px widths, so the
// Workers grid kept its 100% column count and every card line truncated (Mac
// app at 110%, 2026-10-14). Grid minimums are rem now. And a fixed menu placed
// from getBoundingClientRect must land where the rect says, whatever zoom does
// to the two coordinate spaces (the Split menu opened ~500px right of its button).
import { test, expect } from './fixtures';

test('the Workers grid drops columns when font-size zoom grows the text', async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any)._applyZoom === 'function');
  const cols = () => page.evaluate(() => {
    const probe = document.createElement('div');
    probe.className = 'cards grid-mode';
    probe.style.cssText = 'position:absolute;left:0;top:0;width:1280px;visibility:hidden';
    document.body.appendChild(probe);
    const n = getComputedStyle(probe).gridTemplateColumns.split(' ').length;
    probe.remove();
    return n;
  });
  const at100 = await cols();
  await page.evaluate(() => { (window as any)._zoomFontMode = true; // @ts-ignore top-level let in app.js
    _zoomLevel = 150; (window as any)._applyZoom(); });
  const at150 = await cols();
  expect(at100).toBe(4);
  expect(at150, 'the same width holds fewer columns at 150%').toBe(2);
});

test('a fixed menu lands where the button rect says under CSS zoom', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any)._placeFixedAt === 'function');
  const r = await page.evaluate(() => {
    document.documentElement.style.zoom = '1.3';
    const el = document.createElement('div');
    el.style.cssText = 'position:fixed;width:50px;height:20px';
    document.body.appendChild(el);
    (window as any)._placeFixedAt(el, 500, 200, 'spec');
    const b = el.getBoundingClientRect();
    el.remove();
    document.documentElement.style.zoom = '';
    return { left: b.left, top: b.top };
  });
  expect(Math.abs(r.left - 500)).toBeLessThan(2);
  expect(Math.abs(r.top - 200)).toBeLessThan(2);
});

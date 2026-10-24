// The worker view is compact on desktop (Ethan, 2026-10-23: "too large a % of
// the overall screen on worker views"). With a mouse, the title and task share
// one band with the toolbar, and "Send command" sits on the status line. Every
// control stays shown and keeps its size. Phones keep the stacked header.
import { test, expect } from './fixtures';

test('the worker view header is one band on desktop and stacked on a phone', async ({ page, request }, testInfo) => {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function'
    && (globalThis as any).eval('typeof sessions') !== 'undefined');
  await page.evaluate(() => (globalThis as any).eval(
    "localStorage.removeItem('amux_peek_head'); sessions.push({name:'zz-compact',dir:'/tmp',running:true,status:'active',task_name:'Applying namespace manifest to staging'}); openPeek('zz-compact')"));
  await expect(page.locator('#peek-overlay .peek-toolbar')).toBeVisible();
  const g = await page.evaluate(() => {
    const r = (s: string) => { const e = document.querySelector(s) as HTMLElement | null; return e && e.getClientRects().length ? e.getBoundingClientRect() : null; };
    const head = r('#peek-overlay > .overlay-header')!, title = r('#peek-title-row')!, tools = r('#peek-overlay .peek-toolbar')!;
    const status = r('#peek-status'), toggle = r('#peek-cmd-toggle');
    const fine = matchMedia('(min-width: 601px) and (hover: hover) and (pointer: fine)').matches;
    const btns = [...document.querySelectorAll('#peek-overlay .peek-toolbar button')].filter(b => b.getClientRects().length).map(b => b.getBoundingClientRect().height);
    return { fine, head: head.height, sameBand: Math.abs((title.top + title.bottom) / 2 - (tools.top + tools.bottom) / 2) < tools.height / 2 && title.right <= tools.left + 1,
      toggleOnStatus: !!(status && toggle && Math.abs(toggle.top - status.top) < 8), minBtn: Math.min(...btns) };
  });
  testInfo.annotations.push({ type: 'geometry', description: JSON.stringify(g) });
  expect(g.minBtn, 'toolbar controls keep their size').toBeGreaterThanOrEqual(43.9);
  if (g.fine) {
    expect(g.sameBand, 'title and toolbar share one band').toBe(true);
    expect(g.head, 'the header is one band').toBeLessThanOrEqual(62);
    expect(g.toggleOnStatus, '"Send command" sits on the status line').toBe(true);
  } else {
    expect(g.sameBand, 'a touch screen keeps the stacked header').toBe(false);
  }
});

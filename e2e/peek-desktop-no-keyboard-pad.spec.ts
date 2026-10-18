// After macOS Split View the worker page "gets stuck": the composer sat mid-window
// over a blank strip (Ethan, 2026-10-12). Leaving Split View resizes the window,
// and for a moment the visual viewport is shorter than innerHeight; with the
// composer focused, the overlay read that gap as an on-screen keyboard and pinned
// it as padding-bottom. A desktop pointer has no on-screen keyboard.
import {test, expect} from './fixtures';

test('a mid-resize visual viewport never pads the desktop peek like a keyboard', async ({page}, info) => {
  test.skip(info.project.name !== 'desktop', 'touch devices do have an on-screen keyboard');
  const worker = {name: 'pad-worker', running: true, status: 'idle', provider: 'claude', dir: '/tmp'};
  await page.route(/\/api\/sessions(?:\?.*)?$/, r => r.fulfill({json: [worker]}));
  await page.route(/\/api\/sessions\/pad-worker\/peek/, r => r.fulfill({json: {name: 'pad-worker', history: 'hello', live: '❯ ', output: '❯ '}}));
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function');
  await page.evaluate(w => { (0, eval)('sessions=' + JSON.stringify([w]) + '; render();'); (window as any).openPeek(w.name); }, worker);
  await expect(page.locator('#peek-overlay')).toHaveClass(/active/);
  await page.locator('#peek-cmd-input').focus();
  const pad = await page.evaluate(async () => {
    const vv = window.visualViewport!;
    // The transition snapshot: the visual viewport 400px shorter than the window.
    Object.defineProperty(vv, 'height', {configurable: true, get: () => window.innerHeight - 400});
    vv.dispatchEvent(new Event('resize'));
    window.dispatchEvent(new Event('resize'));
    await new Promise(r => setTimeout(r, 600));
    return getComputedStyle(document.getElementById('peek-overlay')!).paddingBottom;
  });
  expect(pad, 'no keyboard padding on a desktop pointer').toBe('0px');
  await expect(page.locator('#peek-overlay')).not.toHaveClass(/vv-compact/);
});

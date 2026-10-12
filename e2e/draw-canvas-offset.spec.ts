// Draw tab: a stroke landed far from the cursor (Ethan, 2026-10-12). The editor
// caches where its canvas sits and re-measures only on its own resize and on
// scroll, so a canvas that MOVES without resizing left every stroke offset by
// the distance it moved. Reads the canvas pixels under the pointer.
import {test, expect} from './fixtures';

test('a stroke lands under the cursor after the canvas moves without resizing', async ({page}, info) => {
  test.skip(info.project.name !== 'desktop', 'mouse drag; the offset cache is the same on every device');
  test.setTimeout(90_000);
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).switchView === 'function');
  await page.evaluate(() => (window as any).switchView('draw'));
  await page.waitForFunction(() => { const d = (0, eval)('_draw'); return !!(d && d.api && d.path && !d.opening); }, null, {timeout: 30_000});
  await page.evaluate(() => (0, eval)('_draw').api.setActiveTool({type: 'freedraw'}));
  // Move the canvas down and right, same size: no resize, no scroll.
  await page.evaluate(() => {
    const w = document.getElementById('draw-canvas-wrap')!;
    w.style.position = 'relative'; w.style.left = '90px'; w.style.top = '70px';
  });
  await page.waitForTimeout(300);
  const box = (await page.locator('#draw-host canvas.interactive').boundingBox())!;
  const a = [box.x + box.width * 0.4, box.y + box.height * 0.4];
  await page.mouse.move(a[0] - 30, a[1]);
  await page.mouse.move(a[0], a[1]);
  await page.waitForTimeout(150);
  await page.mouse.down();
  for (let i = 1; i <= 12; i++) await page.mouse.move(a[0] + 10 * i, a[1]);
  await page.mouse.up();
  await expect.poll(() => page.evaluate(() => (0, eval)('_draw').api.getSceneElements().length)).toBe(1);
  await page.waitForTimeout(300);
  const ink = await page.evaluate(([x, y]) => {
    const c = document.querySelector('#draw-host canvas.static') as HTMLCanvasElement;
    const r = c.getBoundingClientRect(), sx = c.width / r.width, sy = c.height / r.height;
    const d = c.getContext('2d')!.getImageData(Math.round((x - r.left) * sx) - 6, Math.round((y - r.top) * sy) - 6, 12, 12).data;
    let n = 0; for (let i = 3; i < d.length; i += 4) if (d[i] > 40) n++;
    return n;
  }, [a[0] + 60, a[1]]);
  expect(ink, 'ink under the middle of the drag').toBeGreaterThan(0);
  const st = await page.evaluate(() => {
    const s = (0, eval)('_draw').api.getAppState(), r = document.querySelector('#draw-host .excalidraw')!.getBoundingClientRect();
    return {dx: Math.round(r.left - s.offsetLeft), dy: Math.round(r.top - s.offsetTop)};
  });
  expect(st, 'the editor knows where its canvas is').toEqual({dx: 0, dy: 0});
});

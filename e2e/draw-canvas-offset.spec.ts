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

// The amux UI zoom (Settings, e.g. 130%) is CSS zoom on <html>. Excalidraw maps
// pointer coordinates for an unzoomed page, so under it a stroke landed
// 100-200px from the cursor (Ethan, 2026-10-13). The truth is the rendered
// screenshot: dark pixels must lie along the pointer's path. (Coordinates read
// from getBoundingClientRect cannot show it: they agree with the editor's cache
// while the ink is elsewhere.) The root drops CSS zoom while Draw is open and
// gets it back on leaving.
for (const zoom of ['130', '80']) {
  test(`a stroke lands under the cursor at ${zoom}% UI zoom`, async ({page, browser}, info) => {
    test.skip(!info.project.name.startsWith('desktop'), 'desktop applies the UI zoom as CSS zoom; touch devices scale font-size instead');
    test.setTimeout(120_000);
    await page.addInitScript((z) => { localStorage.setItem('amux_walkthrough_done', '1'); localStorage.setItem('amux_zoom', z); }, zoom);
    await page.goto('/');
    await page.waitForFunction(() => typeof (window as any).switchView === 'function');
    expect(await page.evaluate(() => document.documentElement.style.zoom)).toBe(String(Number(zoom) / 100));
    await page.evaluate(() => (window as any).switchView('draw'));
    await page.waitForFunction(() => { const d = (0, eval)('_draw'); return !!(d && d.api && d.path && !d.opening); }, null, {timeout: 30_000});
    expect(await page.evaluate(() => document.documentElement.style.zoom), 'no CSS zoom on the root while Draw is open').toBe('');
    await page.evaluate(() => (0, eval)('_draw').api.setActiveTool({type: 'freedraw'}));
    await page.waitForTimeout(300);
    const box = (await page.locator('#draw-host canvas.interactive').boundingBox())!;
    const y = Math.round(box.y + box.height * 0.5), x0 = Math.round(box.x + box.width * 0.3), x1 = Math.round(box.x + box.width * 0.7);
    await page.mouse.move(x0, y); await page.mouse.down();
    for (let i = 1; i <= 20; i++) await page.mouse.move(x0 + (x1 - x0) * i / 20, y);
    await page.mouse.up();
    await page.waitForTimeout(600);
    const shot = (await page.screenshot()).toString('base64');
    const dec = await browser.newPage();
    const vw = page.viewportSize()!.width;
    const onPath = await dec.evaluate(async ({shot, x0, x1, y, vw}) => {
      const img = new Image(); img.src = 'data:image/png;base64,' + shot; await img.decode();
      const c = document.createElement('canvas'); c.width = img.width; c.height = img.height;
      const g = c.getContext('2d')!; g.drawImage(img, 0, 0);
      const d = g.getImageData(0, 0, c.width, c.height).data;
      const k = img.width / vw;   // screenshot pixels per CSS pixel (2 on a Retina target)
      let n = 0;
      for (let x = x0 + 30; x < x1 - 30; x++) for (let yy = y - 4; yy <= y + 4; yy++) {
        const i = (c.width * Math.round(yy * k) + Math.round(x * k)) * 4; if (d[i] < 90 && d[i + 1] < 90 && d[i + 2] < 90) n++;
      }
      return n;
    }, {shot, x0, x1, y, vw});
    await dec.close();
    expect(onPath, `ink on the pointer's path at ${zoom}% (screenshot pixels)`).toBeGreaterThan((x1 - x0 - 60) * 2);
    await page.evaluate(() => (window as any).switchView('sessions'));
    expect(await page.evaluate(() => document.documentElement.style.zoom), 'the UI zoom comes back on leaving Draw').toBe(String(Number(zoom) / 100));
  });
}

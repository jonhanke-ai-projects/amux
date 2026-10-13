// Draw tab (Ethan, 2026-10-12): freehand diagrams on Excalidraw, autosaved as
// standard .excalidraw files in a chosen folder, a shared component library on
// disk, snap toggles, export, live reload from disk and Send to worker.
//
// Every assertion that matters reads the FILE ON DISK or the request that left
// the page, not the UI's own claim, because the failure this guards against is
// a status line that says "saved" over a write that never happened.
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { cleanup } from './teardown';

type Win = Window & { _draw: any; switchView: (v: string) => void; _AMUX_DATA_DIR: string; _AMUX_AUTH_TOKEN: string };

// Switch Excalidraw's tool and wait until it is active. setActiveTool applies on
// the editor's next state update, so a drag sent straight after it could still
// draw with the previous tool (main CI, 2026-10-13: a rectangle drag drew a
// second freedraw stroke).
async function setTool(page: Page, type: string) {
  await page.evaluate((t) => (window as unknown as Win)._draw.api.setActiveTool({ type: t }), type);
  await page.waitForFunction((t) => (window as unknown as Win)._draw.api.getAppState().activeTool.type === t, type);
}

async function openDraw(page: Page) {
  await page.waitForFunction(() => typeof (window as unknown as Win).switchView === 'function');
  await expect(page.locator('#tab-draw')).toHaveCount(1);
  await page.evaluate(() => (window as unknown as Win).switchView('draw'));
  await expect(page.locator('#draw-view')).toBeVisible();
  // The editor bundle loads lazily on first open, then the API arrives.
  await page.waitForFunction(() => { const d = (window as unknown as Win)._draw; return !!(d && d.api && d.path && !d.opening); }, null, { timeout: 30_000 });
  await expect(page.locator('#draw-host .excalidraw')).toBeVisible();
}

async function canvasBox(page: Page) {
  const box = await page.locator('#draw-host canvas.interactive').boundingBox();
  if (!box) throw new Error('no interactive canvas');
  return box;
}

async function drag(page: Page, from: [number, number], to: [number, number], steps = 8) {
  await page.mouse.move(from[0], from[1]);
  await page.mouse.down();
  for (let i = 1; i <= steps; i++) {
    await page.mouse.move(from[0] + ((to[0] - from[0]) * i) / steps, from[1] + ((to[1] - from[1]) * i) / steps + (i % 2 ? 6 : -6));
  }
  await page.mouse.up();
}

const sceneTypes = (page: Page) => page.evaluate(() =>
  (window as unknown as Win)._draw.api.getSceneElements().map((e: any) => e.type).sort());

async function waitSaved(page: Page) {
  await expect(page.locator('#draw-save-state')).toHaveAttribute('data-state', 'saved', { timeout: 15_000 });
}

test('Draw tab: draw, autosave to disk, reopen, library, snap, export, live reload, send to worker', async ({ page, request }, info) => {
  test.setTimeout(150_000);
  page.on('console', m => { if (/\[draw\]|error/i.test(m.text())) console.log('[page]', m.text().slice(0, 300)); });
  const dir = await mkdtemp(join(tmpdir(), 'amux-draw-'));
  await page.goto('/');
  const headers = await page.evaluate(() => ({ Authorization: 'Bearer ' + (window as unknown as Win)._AMUX_AUTH_TOKEN }));
  const pref = (key: string, value: string) => request.post('/api/prefs', { headers, data: { key, value } });
  const prior = await (await request.get('/api/prefs?key=draw_dir', { headers })).json();
  try {
    expect((await pref('draw_dir', dir)).ok()).toBeTruthy();
    expect((await pref('draw_last', '')).ok()).toBeTruthy();
    expect((await pref('draw_settings', JSON.stringify({ grid: false, align: true, connect: true }))).ok()).toBeTruthy();
    await page.reload();
    await openDraw(page);
    const path = await page.evaluate(() => (window as unknown as Win)._draw.path);
    expect(path, 'a new drawing lands in the chosen folder').toBe(join(dir, 'Untitled.excalidraw'));
    await expect(page.locator('#draw-save-state')).toHaveAttribute('data-state', 'new');

    // A freehand stroke and a rectangle, with real pointer events.
    const box = await canvasBox(page);
    const cx = box.x + box.width / 2, cy = box.y + box.height / 2;
    await setTool(page, 'freedraw');
    await drag(page, [cx - 120, cy - 60], [cx - 20, cy - 10], 12);
    await setTool(page, 'rectangle');
    await drag(page, [cx + 10, cy + 10], [cx + 110, cy + 70]);
    await expect.poll(() => sceneTypes(page)).toEqual(['freedraw', 'rectangle']);

    // Autosave wrote a valid .excalidraw file (read from disk, not the UI).
    await waitSaved(page);
    const onDisk = JSON.parse(await readFile(path, 'utf8'));
    expect(onDisk.type).toBe('excalidraw');
    expect(onDisk.elements.map((e: any) => e.type).sort()).toEqual(['freedraw', 'rectangle']);

    // Reload the page: the tab reopens the same file with both elements.
    await page.reload();
    await openDraw(page);
    expect(await page.evaluate(() => (window as unknown as Win)._draw.path)).toBe(path);
    await expect.poll(() => sceneTypes(page)).toEqual(['freedraw', 'rectangle']);

    // Snap toggles drive Excalidraw's own state and persist per user.
    const snap = () => page.evaluate(() => { const s = (window as unknown as Win)._draw.api.getAppState();
      return { grid: s.gridModeEnabled, align: s.objectsSnapModeEnabled, connect: s.isBindingEnabled }; });
    expect(await snap()).toEqual({ grid: false, align: true, connect: true });
    await page.locator('#draw-snap-grid').click();
    await page.locator('#draw-snap-align').click();
    await page.locator('#draw-snap-connect').click();
    await expect.poll(snap).toEqual({ grid: true, align: false, connect: false });
    await expect(page.locator('#draw-snap-grid')).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(async () => JSON.parse((await (await request.get('/api/prefs?key=draw_settings', { headers })).json()).value))
      .toEqual({ grid: true, align: false, connect: false });
    await page.locator('#draw-snap-grid').click();
    await page.locator('#draw-snap-connect').click();

    // Save the rectangle to the library: the .excalidrawlib on disk has it.
    await page.evaluate(() => { const api = (window as unknown as Win)._draw.api;
      const rect = api.getSceneElements().find((e: any) => e.type === 'rectangle');
      api.updateScene({ appState: { selectedElementIds: { [rect.id]: true } } }); });
    const mobile = (page.viewportSize()?.width || 1280) <= 700;
    if (mobile) await page.locator('#draw-lib-open').click();
    await page.locator('#draw-lib-save').click();
    await page.locator('#modal-prompt-input').fill('Service');
    await page.locator('#modal-prompt-input').press('Enter');
    const libPath = join(await page.evaluate(() => (window as unknown as Win)._AMUX_DATA_DIR), 'draw', 'library.excalidrawlib');
    await expect.poll(async () => { try { return JSON.parse(await readFile(libPath, 'utf8')).libraryItems.map((i: any) => i.name); } catch { return null; } },
      { timeout: 10_000 }).toEqual(['Service']);
    const item = page.locator('.draw-lib-item').filter({ hasText: 'Service' });
    await expect(item).toBeVisible();

    // ...and bring it back onto the canvas: drag on desktop, tap on a phone.
    const before = (await sceneTypes(page)).length;
    if (mobile) await item.click();
    else await item.dragTo(page.locator('#draw-host canvas.interactive'), { targetPosition: { x: box.width / 2 + 160, y: box.height / 2 - 120 } });
    await expect.poll(() => sceneTypes(page)).toHaveLength(before + 1);
    await waitSaved(page);
    expect(JSON.parse(await readFile(path, 'utf8')).elements.filter((e: any) => !e.isDeleted && e.type === 'rectangle')).toHaveLength(2);

    // Export PNG and SVG as real downloads.
    await page.evaluate(() => (window as unknown as Win)._draw.api.updateScene({ appState: { selectedElementIds: {} } }));
    for (const [kind, label, check] of [
      ['png', 'PNG image', (b: Buffer) => b.subarray(1, 4).toString() === 'PNG'],
      ['svg', 'SVG image', (b: Buffer) => b.toString().includes('<svg')],
    ] as const) {
      await page.locator('#draw-export-btn').click();
      const dl = page.waitForEvent('download');
      await page.locator('#draw-export-menu button', { hasText: label }).click();
      const d = await dl;
      expect(d.suggestedFilename()).toBe('Untitled.' + kind);
      const bytes = await readFile(await d.path());
      expect(check(bytes), kind + ' bytes').toBe(true);
    }

    // Live reload: a worker rewrites the file on disk; the canvas follows.
    const external = JSON.parse(JSON.stringify(onDisk));
    external.elements = [Object.assign({}, onDisk.elements.find((e: any) => e.type === 'rectangle'), { id: 'from-disk', type: 'ellipse', version: 1 })];
    await new Promise(r => setTimeout(r, 1100));   // the server's ETag has 1s granularity
    await writeFile(path, JSON.stringify(external));
    await expect.poll(() => sceneTypes(page), { timeout: 10_000 }).toEqual(['ellipse']);
    await expect(page.locator('#draw-save-state')).toHaveAttribute('data-state', 'saved');

    // Send to worker: one PNG through the upload API, then the composer's send
    // path with the @upload reference and the drawing's own path.
    const sent: any[] = [];
    await page.route('**/api/sessions/*/send', async route => {
      sent.push({ url: route.request().url(), body: JSON.parse(route.request().postData() || '{}') });
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ok: true }) });
    });
    await page.locator('.draw-act[aria-label="Send to worker"]').click();
    await page.locator('#draw-send-worker').fill('draw-e2e-worker');
    await page.locator('#draw-send-prompt').fill('build this');
    await page.locator('#modal-btns .btn.primary').click();
    await expect.poll(() => sent.length, { timeout: 15_000 }).toBe(1);
    expect(sent[0].url).toContain('/api/sessions/draw-e2e-worker/send');
    const text: string = sent[0].body.text;
    expect(text).toContain('build this');
    const upload = text.match(/@(\S+\.png)/);
    expect(upload, text).not.toBeNull();
    expect(upload![1]).toContain('/uploads/');
    expect((await readFile(upload![1])).subarray(1, 4).toString()).toBe('PNG');
    expect(text).toContain('@' + path);

    // A refused write is never reported as saved.
    await page.route('**/api/file', route => route.request().method() === 'PUT'
      ? route.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ error: 'disk full' }) })
      : route.continue());
    await setTool(page, 'rectangle');
    await drag(page, [cx - 100, cy + 40], [cx - 40, cy + 90]);
    await expect(page.locator('#draw-save-state')).toHaveAttribute('data-state', 'error', { timeout: 10_000 });
    await expect(page.locator('#draw-save-state')).toHaveText('Not saved');
    expect(JSON.parse(await readFile(path, 'utf8')).elements.map((e: any) => e.type)).toEqual(['ellipse']);
  } finally {
    await cleanup('restore draw_dir', () => pref('draw_dir', prior.value || ''), info);
    await cleanup('clear draw_last', () => pref('draw_last', ''), info);
    await cleanup('remove temp folder', () => rm(dir, { recursive: true, force: true }), info);
  }
});

test('Draw tab: a disk change over unsaved edits asks first; .excalidraw files from Files open in Draw', async ({ page, request }, info) => {
  test.setTimeout(120_000);
  const dir = await mkdtemp(join(tmpdir(), 'amux-draw-c-'));
  const path = join(dir, 'shared.excalidraw');
  const scene = (types: string[]) => JSON.stringify({ type: 'excalidraw', version: 2, source: 'e2e', appState: {}, files: {},
    elements: types.map((t, i) => ({ id: 'e' + i, type: t, x: i * 150, y: 0, width: 100, height: 60, angle: 0, strokeColor: '#1e1e1e',
      backgroundColor: 'transparent', fillStyle: 'solid', strokeWidth: 2, strokeStyle: 'solid', roughness: 1, opacity: 100,
      groupIds: [], frameId: null, roundness: null, seed: i + 1, version: 1, versionNonce: i + 1, isDeleted: false,
      boundElements: null, updated: 1, link: null, locked: false })) });
  await writeFile(path, scene(['rectangle']));
  await page.goto('/');
  const headers = await page.evaluate(() => ({ Authorization: 'Bearer ' + (window as unknown as Win)._AMUX_AUTH_TOKEN }));
  const pref = (key: string, value: string) => request.post('/api/prefs', { headers, data: { key, value } });
  try {
    await pref('draw_dir', dir);
    await pref('draw_last', '');
    await page.reload();
    await page.waitForFunction(() => typeof (window as unknown as Win).switchView === 'function');
    // The Files tab hands a drawing to Draw instead of the text viewer.
    await page.evaluate(() => (window as unknown as Win).switchView('files'));
    await page.evaluate(p => (window as any).openFilePreview(p), path);
    await expect(page.locator('#draw-view')).toBeVisible();
    await page.waitForFunction(p => { const d = (window as unknown as Win)._draw; return !!(d && d.api && d.path === p && !d.opening); }, path, { timeout: 30_000 });
    await expect.poll(() => sceneTypes(page)).toEqual(['rectangle']);
    await expect(page.locator('#file-overlay')).not.toHaveClass(/active/);

    // Unsaved local work (the server refuses the write), then a worker rewrites the file.
    await page.route('**/api/file', route => route.request().method() === 'PUT'
      ? route.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ error: 'disk full' }) })
      : route.continue());
    const box = await canvasBox(page);
    await setTool(page, 'diamond');
    await drag(page, [box.x + box.width / 2 - 40, box.y + box.height / 2 + 60], [box.x + box.width / 2 + 40, box.y + box.height / 2 + 120]);
    await expect(page.locator('#draw-save-state')).toHaveAttribute('data-state', 'error', { timeout: 10_000 });
    await new Promise(r => setTimeout(r, 1100));
    await writeFile(path, scene(['ellipse', 'ellipse']));
    // Not silently reloaded over the local edit: the user is asked.
    await expect(page.locator('#draw-conflict')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('#draw-save-state')).toHaveAttribute('data-state', 'conflict');
    expect((await sceneTypes(page)).sort()).toEqual(['diamond', 'rectangle']);
    await page.locator('#draw-conflict button', { hasText: 'Load disk version' }).click();
    await expect.poll(() => sceneTypes(page)).toEqual(['ellipse', 'ellipse']);
    await expect(page.locator('#draw-conflict')).toBeHidden();
  } finally {
    await cleanup('clear draw_dir', () => pref('draw_dir', ''), info);
    await cleanup('clear draw_last', () => pref('draw_last', ''), info);
    await cleanup('remove temp folder', () => rm(dir, { recursive: true, force: true }), info);
  }
});

test('Draw tab fits a phone: no horizontal overflow, library and actions reachable', async ({ page, request }, info) => {
  test.setTimeout(90_000);
  const vw = page.viewportSize()?.width || 1280;
  test.skip(vw > 700, 'phone layout only');
  const dir = await mkdtemp(join(tmpdir(), 'amux-draw-m-'));
  await page.goto('/');
  const headers = await page.evaluate(() => ({ Authorization: 'Bearer ' + (window as unknown as Win)._AMUX_AUTH_TOKEN }));
  const pref = (key: string, value: string) => request.post('/api/prefs', { headers, data: { key, value } });
  try {
    await pref('draw_dir', dir);
    await pref('draw_last', '');
    await page.reload();
    await openDraw(page);
    const m = await page.evaluate(() => ({ sw: document.documentElement.scrollWidth, iw: innerWidth,
      host: document.getElementById('draw-host')!.getBoundingClientRect().width }));
    expect(m.sw, JSON.stringify(m)).toBeLessThanOrEqual(m.iw);
    expect(m.host, 'the canvas spans the screen').toBeGreaterThan(m.iw - 2);
    // Every amux control in the Draw chrome is a 44px touch target.
    const small = await page.evaluate(() => Array.from(document.querySelectorAll('#draw-view .draw-head button, #draw-view .draw-snapbar button'))
      .filter(b => (b as HTMLElement).offsetParent)
      .map(b => { const r = b.getBoundingClientRect(); return { id: b.id || b.getAttribute('aria-label') || b.textContent, w: r.width, h: r.height }; })
      .filter(r => r.w < 44 || r.h < 44));
    expect(small).toEqual([]);
    // The library is a bottom sheet behind a button.
    await expect(page.locator('#draw-lib')).not.toBeInViewport();
    await page.locator('#draw-lib-open').click();
    await expect(page.locator('#draw-lib')).toBeInViewport();
    await expect(page.locator('#draw-lib-save')).toBeVisible();
    await page.locator('#draw-lib-scrim').click({ position: { x: 20, y: 20 } });
    await expect(page.locator('#draw-lib')).not.toBeInViewport();
    // Export and Send are reachable.
    await page.locator('#draw-export-btn').click();
    await expect(page.locator('#draw-export-menu')).toBeVisible();
    const menu = await page.locator('#draw-export-menu').boundingBox();
    expect(menu!.x).toBeGreaterThanOrEqual(0);
    expect(menu!.x + menu!.width).toBeLessThanOrEqual(vw + 1);
  } finally {
    await cleanup('clear draw_dir', () => pref('draw_dir', ''), info);
    await cleanup('clear draw_last', () => pref('draw_last', ''), info);
    await cleanup('remove temp folder', () => rm(dir, { recursive: true, force: true }), info);
  }
});

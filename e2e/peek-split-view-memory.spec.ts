import { test, expect } from './fixtures';
import type { Page, FrameLocator } from '@playwright/test';

// Ethan, 2026-10-12: Split > Chat rendered a whole second worker view (tab bar,
// Terminal, and its own split), recursively. And: "if I have Split View in
// worker A then return to worker A it should resume that view".
//
// The side panel is an iframe running a second copy of the app. It shares this
// origin's sessionStorage/localStorage with the page around it, read back the
// outer page's peekState (split: true) on boot, and reopened the split inside
// itself, which loaded another iframe, and so on.

const A = 'e2e-split-a';
const B = 'e2e-split-b';
const worker = (name: string) => ({
  name, dir: '/tmp/', provider: 'claude', status: 'idle', running: true, tags: ['e2e'],
  desc: 'split view fixture', pinned: false, isolated: false,
});

async function boot(page: Page) {
  await page.route('**/api/sessions', (route) => route.fulfill({ json: [worker(A), worker(B)] }));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function');
  await page.waitForFunction(({ a, b }) => {
    const rows = JSON.parse(eval('JSON.stringify(sessions)'));
    return rows.some((r: any) => r.name === a) && rows.some((r: any) => r.name === b);
  }, { a: A, b: B });
}

async function open(page: Page, name: string) {
  await page.evaluate((n) => (window as any).openPeek(n), name);
  await expect(page.locator('#peek-overlay')).toHaveAttribute('data-session', name);
}

// Count everything that would betray nesting, on the top page and inside the
// side-panel frame. Only VISIBLE tab bars count: the top page has exactly one.
async function nesting(page: Page) {
  const top = await page.evaluate(() => ({
    tabBars: Array.from(document.querySelectorAll('.peek-tabs'))
      .filter((el) => (el as HTMLElement).offsetParent !== null).length,
    splitWraps: document.querySelectorAll('#peek-split-wrap.split-active').length,
  }));
  return top;
}

async function frameState(frame: FrameLocator) {
  await expect(frame.locator('#peek-overlay')).toHaveClass(/active/);
  // Give a nested split the time it took to mount on the old code (its
  // restore ran 500ms after boot) before counting.
  await frame.locator('body').evaluate(() => new Promise((r) => setTimeout(r, 1200)));
  return {
    panelMode: /peek-embed-panel/.test(await frame.locator('body').getAttribute('class') || ''),
    visibleTabBars: await frame.locator('.peek-tabs:visible').count(),
    splitPanes: await frame.locator('#peek-split-files').count(),
    splitHandles: await frame.locator('#peek-split-handle').count(),
    splitToggles: await frame.locator('#peek-split-toggle').count(),
    activeSplits: await frame.locator('#peek-split-wrap.split-active').count(),
    iframes: await frame.locator('iframe').count(),
  };
}

const NO_NESTING = { panelMode: true, visibleTabBars: 0, splitPanes: 0, splitHandles: 0, splitToggles: 0, activeSplits: 0, iframes: 0 };

test.describe('desktop split', () => {
  test.skip(({ viewport }) => !!viewport && viewport.width <= 600, 'a phone has no split: the button opens Files');

  for (const view of ['chat', 'messages'] as const) {
    test(`Split > ${view} renders one panel, never a nested worker view`, async ({ page }) => {
      await boot(page);
      await open(page, A);
      await page.locator('#peek-split-toggle').click();
      await page.locator('#psp-menu button', { hasText: view === 'chat' ? 'Chat' : 'Messages' }).click();
      await expect(page.locator('#peek-split-wrap')).toHaveClass(/split-active/);
      await expect(page.locator('#psp-frame')).toHaveAttribute('src', new RegExp('peekTab=' + view + '&peekSplitChild=1'));
      const frame = page.frameLocator('#psp-frame');
      expect(await frameState(frame)).toEqual(NO_NESTING);
      expect(await nesting(page)).toEqual({ tabBars: 1, splitWraps: 1 });

      // After a re-render cycle the answer is the same.
      await page.evaluate(() => (window as any).fetchSessions());
      await page.waitForTimeout(600);
      expect(await frameState(frame)).toEqual(NO_NESTING);
      expect(await nesting(page)).toEqual({ tabBars: 1, splitWraps: 1 });

      // Asking the embedded peek for a split directly is refused and beaconed.
      const beacon = page.waitForRequest((r) => r.url().includes('/api/client-debug')
        && (r.postData() || '').includes('"kind":"split-view"'));
      await frame.locator('body').evaluate(() => (window as any).togglePeekSplit(null, true));
      const sent = JSON.parse((await beacon).postData() || '{}');
      expect(sent).toMatchObject({ kind: 'split-view', verdict: 'nested_split_refused', measured: true });
      expect(await frameState(frame)).toEqual(NO_NESTING);
    });
  }

  test('Split > Files shows one tab bar and no frame', async ({ page }) => {
    await boot(page);
    await open(page, A);
    await page.locator('#peek-split-toggle').click();
    await page.locator('#psp-menu button', { hasText: 'Files' }).click();
    await expect(page.locator('#peek-split-wrap')).toHaveClass(/split-active/);
    await expect(page.locator('#psp-frame')).toBeHidden();
    expect(await nesting(page)).toEqual({ tabBars: 1, splitWraps: 1 });
    expect(await page.locator('iframe#psp-frame[src]').count()).toBe(0);
  });

  // Only the iframe the split builds (&peekSplitChild=1) is a side panel. A
  // plain ?peekEmbed= page (Workspace tile, embedded chat) keeps its Files split.
  test('a plain ?peekEmbed= page still opens the Files split', async ({ page }) => {
    await page.route('**/api/sessions', (route) => route.fulfill({ json: [worker(A), worker(B)] }));
    await page.goto('/?peekEmbed=' + A);
    await expect(page.locator('#peek-overlay')).toHaveAttribute('data-session', A);
    await expect(page.locator('body')).not.toHaveClass(/peek-embed-panel/);
    expect(await page.locator('#peek-split-files').count()).toBe(1);
    await page.evaluate(() => (window as any)._peekSplitOpen('files'));
    await expect(page.locator('#peek-split-wrap')).toHaveClass(/split-active/);
    await expect(page.locator('#psf-body')).toBeVisible();
  });

  test('each worker keeps its own view: A split+Chat, B default, reload restores', async ({ page }) => {
    await boot(page);
    await open(page, A);
    await page.locator('#peek-split-toggle').click();
    await page.locator('#psp-menu button', { hasText: 'Chat' }).click();
    await expect(page.locator('#peek-split-wrap')).toHaveClass(/split-active/);
    // Drag-free ratio change through the same path the divider's dblclick takes.
    await page.locator('#peek-split-handle').dblclick();

    await open(page, B);
    await expect(page.locator('#peek-split-wrap')).not.toHaveClass(/split-active/);
    await expect(page.locator('#peek-tab-terminal')).toHaveClass(/active/);
    await expect(page.locator('#psp-frame')).not.toHaveAttribute('src', /.+/);
    // B picks a different main tab; it must not bleed into A.
    await page.evaluate(() => (window as any).setPeekTab('steering'));

    await open(page, A);
    await expect(page.locator('#peek-split-wrap')).toHaveClass(/split-active/);
    await expect(page.locator('.psp-opt[data-psp="chat"]')).toHaveClass(/active/);
    await expect(page.locator('#psp-frame')).toHaveAttribute('src', /peekTab=chat/);
    await expect(page.locator('#peek-tab-terminal')).toHaveClass(/active/);
    expect(await frameState(page.frameLocator('#psp-frame'))).toEqual(NO_NESTING);

    await open(page, B);
    await expect(page.locator('#peek-split-wrap')).not.toHaveClass(/split-active/);
    await expect(page.locator('#peek-tab-steering')).toHaveClass(/active/);

    // Back on A, then reload: A comes back exactly as left.
    await open(page, A);
    await page.reload();
    await page.waitForFunction(() => typeof (window as any).openPeek === 'function');
    await expect(page.locator('#peek-overlay')).toHaveAttribute('data-session', A, { timeout: 10_000 });
    await expect(page.locator('#peek-split-wrap')).toHaveClass(/split-active/);
    await expect(page.locator('#psp-frame')).toHaveAttribute('src', /peekTab=chat/);
    expect(await frameState(page.frameLocator('#psp-frame'))).toEqual(NO_NESTING);
    expect(await nesting(page)).toEqual({ tabBars: 1, splitWraps: 1 });

    // And the store is keyed per worker.
    const views = await page.evaluate(() => JSON.parse(localStorage.getItem('amux_peek_views') || '{}'));
    expect(views[A]).toMatchObject({ split: true, splitView: 'chat', tab: 'terminal' });
    expect(views[B]).toMatchObject({ split: false, tab: 'steering' });
  });

  test('a saved tab the worker no longer has falls back to Terminal', async ({ page }) => {
    await boot(page);
    await page.evaluate((a) => localStorage.setItem('amux_peek_views',
      JSON.stringify({ [a]: { tab: 'no-such-tab', split: false, ts: Date.now() } })), A);
    await open(page, A);
    await expect(page.locator('#peek-tab-terminal')).toHaveClass(/active/);
  });

  test('the store keeps at most 50 workers, newest first', async ({ page }) => {
    await boot(page);
    await page.evaluate(() => {
      const all: Record<string, any> = {};
      for (let i = 0; i < 60; i++) all['old-' + i] = { tab: 'terminal', split: false, ts: 1000 + i };
      localStorage.setItem('amux_peek_views', JSON.stringify(all));
    });
    await open(page, A);
    await page.evaluate(() => (window as any).setPeekTab('steering'));
    const keys = await page.evaluate(() => Object.keys(JSON.parse(localStorage.getItem('amux_peek_views') || '{}')));
    expect(keys.length).toBe(50);
    expect(keys).toContain(A);
    expect(keys).not.toContain('old-0');
  });
});

test.describe('phone', () => {
  test.skip(({ viewport }) => !viewport || viewport.width > 600, 'phone-width behaviour');

  test('a saved desktop split never opens on a phone, tabs still follow the worker, no overflow', async ({ page }) => {
    await boot(page);
    await page.evaluate(({ a }) => localStorage.setItem('amux_peek_views',
      JSON.stringify({ [a]: { tab: 'terminal', split: true, splitView: 'chat', ts: Date.now() } })), { a: A });
    await open(page, A);
    await expect(page.locator('#peek-split-wrap')).not.toHaveClass(/split-active/);
    expect(await page.locator('iframe#psp-frame[src]').count()).toBe(0);
    await page.evaluate(() => (window as any).setPeekTab('steering'));
    await open(page, B);
    await expect(page.locator('#peek-tab-terminal')).toHaveClass(/active/);
    await open(page, A);
    await expect(page.locator('#peek-tab-steering')).toHaveClass(/active/);
    // The phone kept the desktop's split choice rather than erasing it.
    const views = await page.evaluate(() => JSON.parse(localStorage.getItem('amux_peek_views') || '{}'));
    expect(views[A]).toMatchObject({ split: true, splitView: 'chat', tab: 'steering' });
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow).toBeLessThanOrEqual(0);
  });
});

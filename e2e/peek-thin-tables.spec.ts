// A wide table in a THIN peek (Claude on the normal screen, tmux scrollback)
// keeps its rows intact (Ethan, 2026-10-11, gtm-engine's "Inbound audit"
// table). The thin pipeline had no table scroller, so a 173-column table in a
// view about 170 columns wide wrapped every row: borders broke two characters
// from the end and cells spilled onto the next line. The frame below is that
// table, byte for byte, served the way the server sends a thin peek.
import { test, expect } from './fixtures';

const row = (a: string, b: string, c: string) => `  │ ${a.padEnd(59)} │ ${b.padEnd(32)} │ ${c.padEnd(70)} │`;
const rule = (l: string, m: string, r: string) => `  ${l}${'─'.repeat(61)}${m}${'─'.repeat(34)}${m}${'─'.repeat(72)}${r}`;
const TABLE = [
  rule('┌', '┬', '┐'),
  row('                            Who', '            Waiting', '                                 Note'),
  rule('├', '┼', '┤'),
  row('josh.yorke-armstrong@salad.com ("mixpeek\'s cost per frame")', '18 days', 'Its card was discarded automatically for age, so nothing tracks it now'),
  rule('├', '┼', '┤'),
  row('misha@editstock.com', '6 days', 'A different address from the EditStock thread you answered'),
  rule('└', '┴', '┘'),
];
const HISTORY = ['  a) Inbound audit, last 7 days', '', '  Of 9 inbound threads, 5 were answered. These are still open:', '', ...TABLE, ''].join('\n');

test('a wide table in a thin peek scrolls sideways instead of wrapping', async ({ page }) => {
  expect(TABLE.every((l) => [...l].length === 173)).toBe(true);
  const worker = { name: 'thin-table', provider: 'claude', model: 'sonnet', running: true, status: 'idle', dir: '/tmp' };
  await page.setViewportSize({ width: 1280, height: 860 });
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.route(/\/api\/sessions(?:\?.*)?$/, (r) => r.fulfill({ json: [worker] }));
  await page.route(/\/api\/sessions\/thin-table\/peek/, (r) => r.fulfill({ json: {
    name: 'thin-table', history: HISTORY, live: '❯ ', output: '❯ ', pane_cols: 220, pane_rows: 50,
    history_source: 'tmux-scrollback', render: { path: 'tmux-scrollback', steps: ['tmux-scrollback'] },
  } }));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function');
  await page.evaluate((w) => { eval('sessions=' + JSON.stringify([w]) + '; render();'); (window as any).openPeek(w.name); }, worker);
  await expect(page.locator('#peek-body')).toContainText('Inbound audit');

  const box = page.locator('#peek-body .peek-box').filter({ hasText: 'josh.yorke-armstrong' });
  await expect(box).toHaveCount(1);
  const m = await box.evaluate((el) => {
    const cs = getComputedStyle(el);
    const lh = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.2;
    return { lines: el.textContent!.split('\n').length, height: el.scrollHeight, lh, scrollW: el.scrollWidth, clientW: el.clientWidth, ws: cs.whiteSpace };
  });
  expect(m.lines).toBe(TABLE.length);
  // One visual line per table line: nothing wrapped.
  expect(m.height).toBeLessThan(m.lh * (TABLE.length + 1));
  expect(m.ws).toBe('pre');
  // Wider than the view, so it scrolls sideways rather than reflowing.
  expect(m.scrollW).toBeGreaterThan(m.clientW);
  // The row's closing border stays on the row.
  expect(await box.textContent()).toContain('tracks it now │');
});

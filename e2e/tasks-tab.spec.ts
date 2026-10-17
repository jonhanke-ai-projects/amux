import { test, expect } from './fixtures';

/**
 * Ethan, 2026-10-17: "make a new tab ... called 'tasks' and it should be all the
 * tasks in sequential order with each worker tagged, each one clickable and any
 * other data associated with it (messages sent, board issues, etc.)".
 *
 * Against the real server: three tasks are sent to one worker in order, the
 * first and third are linked to the same board card. The Tasks tab lists them
 * oldest first and numbered, each tagged with the worker; opening the first
 * shows its delivery, the card, and the third task as another message on that
 * card; the card opens on the board.
 */
test('Tasks lists every task in order, tagged with its worker, and a task opens its card and related messages', async ({ page }) => {
  const nonce = Date.now().toString(36);
  const worker = 'e2e-tasks-' + nonce;
  const texts = ['first task ' + nonce, 'second task ' + nonce, 'third task ' + nonce];
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).switchView === 'function', { timeout: 20000 });
  // Seeded through the page's own authenticated fetch, one after another so
  // the server stamps them in order.
  const seed = await page.evaluate(async ({ worker, texts }) => {
    const ids: number[] = [];
    for (const text of texts) {
      const r = await fetch('/api/history', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text, type: 'user', session: worker }) });
      ids.push((await r.json()).id);
      await new Promise(res => setTimeout(res, 20));
    }
    const c = await fetch('/api/board', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ title: 'Tasks tab card ' + worker, status: 'todo', session: worker }) });
    const card = (await c.json()).id;
    const link = async (id: number) => (await fetch('/api/history/MSG-' + id + '/card', { method: 'PUT', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ session: worker, card_id: card, reason: 'e2e tasks tab' }) })).status;
    return { ids, card, links: [await link(ids[0]), await link(ids[2])] };
  }, { worker, texts });
  expect(seed.ids.every(Boolean), JSON.stringify(seed)).toBe(true);
  expect(seed.card, JSON.stringify(seed)).toBeTruthy();
  expect(seed.links, JSON.stringify(seed)).toEqual([200, 200]);

  await page.evaluate(() => (window as any).switchView('tasks'));
  await expect(page.locator('#tab-tasks')).toHaveClass(/active/);
  await expect(page.locator('#tasks-view')).toBeVisible();
  // Narrow to this worker the way a person would: from the worker menu.
  const fetched = page.waitForRequest(r => r.url().includes('/api/history?') && r.url().includes('order=asc') && r.url().includes('session=' + encodeURIComponent(worker)));
  await page.evaluate((w) => (window as any)._tasksByWorker(w), worker);
  await fetched;

  const items = page.locator('#tasks-list .task-item');
  await expect(items).toHaveCount(3, { timeout: 15000 });
  for (let i = 0; i < 3; i++) {
    await expect(items.nth(i)).toContainText(texts[i]);
    await expect(items.nth(i).locator('.task-num')).toHaveText('#' + (i + 1));
    await expect(items.nth(i).locator('.msg-sess-link')).toHaveText(worker);
  }
  await expect(page.locator('#tasks-count')).toContainText('of 3 for ' + worker);

  // Opening the first task shows its facts, its card and the other message on that card.
  await items.nth(0).locator('.task-row .msg-body').click();
  const detail = page.locator('#tasks-detail-' + seed.ids[0]);
  await expect(detail).toBeVisible();
  await expect(detail).toContainText('Delivery');
  await expect(detail).toContainText('MSG-' + seed.ids[0]);
  await expect(detail.locator('.task-card-open')).toContainText(String(seed.card));
  await expect(detail.locator('.task-card-msgs')).toContainText('MSG-' + seed.ids[2]);
  await expect(detail.locator('.task-card-msgs')).not.toContainText('MSG-' + seed.ids[0]);
  // The second task has no card and says so.
  await items.nth(1).locator('.task-row .msg-body').click();
  await expect(page.locator('#tasks-detail-' + seed.ids[1])).toContainText('No board card is linked');
  // Clicking a task again closes it.
  await items.nth(0).locator('.task-row .msg-body').click();
  await expect(detail).toBeHidden();

  // The card opens on the board.
  await items.nth(0).locator('.task-row .msg-body').click();
  await detail.locator('.task-card-open').click();
  await expect(page.locator('#tab-board')).toHaveClass(/active/);
  await expect(page.locator('body')).toContainText('Tasks tab card ' + worker, { timeout: 10000 });
});

test("a worker's Tasks tab lists only that worker's tasks, in order", async ({ page }) => {
  const nonce = Date.now().toString(36);
  const worker = 'e2e-probe';
  const texts = ['probe task one ' + nonce, 'probe task two ' + nonce];
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function', { timeout: 20000 });
  const ok = await page.evaluate(async ({ worker, texts }) => {
    const out: number[] = [];
    for (const text of texts) {
      out.push((await fetch('/api/history', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text, type: 'user', session: worker }) })).status);
      await new Promise(res => setTimeout(res, 20));
    }
    // Another worker's task must not appear in this worker's tab.
    out.push((await fetch('/api/history', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text: 'elsewhere ' + texts[0], type: 'user', session: 'e2e-tasks-other' }) })).status);
    return out;
  }, { worker, texts });
  expect(ok.every(s => s === 200), String(ok)).toBe(true);

  await page.evaluate((w) => (window as any).openPeek(w), worker);
  await expect(page.locator('#peek-overlay')).toBeVisible();
  await page.locator('#peek-tab-tasks').click();
  await expect(page.locator('#peek-tab-tasks')).toHaveClass(/active/);
  const panel = page.locator('#peek-tasks-panel');
  await expect(panel).toBeVisible();
  const items = panel.locator('.task-item');
  await expect(items.last()).toContainText(texts[1], { timeout: 15000 });
  const n = await items.count();
  await expect(items.nth(n - 2)).toContainText(texts[0]);
  const nums = await items.locator('.task-num').allTextContents();
  const seq = nums.map(t => parseInt(t.replace('#', ''), 10));
  expect(seq, 'numbered in order').toEqual(seq.slice().sort((a, b) => a - b));
  for (const tag of await items.locator('.msg-sess-link').allTextContents()) expect(tag).toBe(worker);
  await expect(panel).not.toContainText('elsewhere ' + texts[0]);
  // A task opens its detail inside the worker's tab too.
  const lastId = await items.last().getAttribute('data-task-id');
  await items.last().locator('.task-row .msg-body').click();
  await expect(page.locator('#peek-tasks-detail-' + lastId)).toContainText('Delivery');
});

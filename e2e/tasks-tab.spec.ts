import { test, expect, type Page } from './fixtures';

/**
 * Ethan, 2026-10-17: "make a new tab ... called 'tasks' and it should be all the
 * tasks in sequential order with each worker tagged, each one clickable and any
 * other data associated with it", then "tasks still look to be displaying
 * messages not tasks".
 *
 * A task is what a worker is working on: the "Task:" line on its card, recorded
 * each time it changes. Against the real server a new test worker takes two tasks
 * from its board (one card in doing, then the next), and receives a message
 * while on the second. The Tasks tab lists the two tasks, never the message;
 * the second task's detail holds the message and its card.
 */
async function seedTwoTasks(page: Page, nonce: string) {
  const worker = 'e2e-tasks-' + nonce;
  return page.evaluate(async ({ worker, nonce }) => {
    const j = async (r: Response) => ({ status: r.status, body: await r.json().catch(() => ({})) });
    const post = (path: string, body: object) => fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }).then(j);
    const patch = (path: string, body: object) => fetch(path, { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }).then(j);
    const recorded = async (n: number) => {
      for (let i = 0; i < 40; i++) {
        await fetch('/api/sessions');
        const d = await (await fetch('/api/task-history?session=' + encodeURIComponent(worker) + '&q=' + encodeURIComponent(nonce))).json();
        if ((d.total || 0) >= n) return d.total;
        await new Promise(res => setTimeout(res, 500));
      }
      return -1;
    };
    // A stopped worker is enough: its task line comes from its doing card.
    const mk = await post('/api/sessions', { name: worker, dir: '/tmp', provider: 'codex' });
    const a = await post('/api/board', { title: 'First task ' + nonce, status: 'doing', session: worker });
    const first = await recorded(1);
    await patch('/api/board/' + a.body.id, { status: 'done', evidence: 'none: e2e seed' });
    const b = await post('/api/board', { title: 'Second task ' + nonce, status: 'doing', session: worker });
    const second = await recorded(2);
    const msg = await post('/api/history', { text: 'a message during the second task ' + nonce, type: 'user', session: worker });
    return { worker, cards: [a.body.id, b.body.id], statuses: [mk.status, a.status, b.status], first, second, msg: msg.status };
  }, { worker, nonce });
}

test('Tasks lists the tasks a worker took on, in order, and a task opens its card and messages', async ({ page }) => {
  const nonce = Date.now().toString(36);
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).switchView === 'function', { timeout: 20000 });
  const seed = await seedTwoTasks(page, nonce);
  expect(seed.statuses, JSON.stringify(seed)).toEqual([201, 201, 201]);
  expect([seed.first, seed.second], JSON.stringify(seed)).toEqual([1, 2]);

  await page.locator('#tab-tasks').click();
  await expect(page.locator('#tasks-view')).toBeVisible();
  const fetched = page.waitForResponse(r => r.url().includes('/api/task-history?') && r.url().includes('q=' + nonce));
  await page.locator('#tasks-search').fill(nonce);
  await fetched;

  const items = page.locator('#tasks-list .task-item');
  await expect(items).toHaveCount(2, { timeout: 15000 });
  await expect(items.nth(0).locator('.task-title')).toHaveText('First task ' + nonce);
  await expect(items.nth(1).locator('.task-title')).toHaveText('Second task ' + nonce);
  await expect(items.nth(0).locator('.task-num')).toHaveText('#1');
  await expect(items.nth(1).locator('.task-num')).toHaveText('#2');
  for (let i = 0; i < 2; i++) await expect(items.nth(i).locator('.msg-sess-link')).toHaveText(seed.worker);
  await expect(items.nth(0)).toContainText(/ran \d/);
  await expect(items.nth(1)).toContainText('current');
  // A message is never a task row.
  await expect(page.locator('#tasks-list')).not.toContainText('a message during the second task');

  // The second task opens: started, current, its card and the message it received.
  await items.nth(1).locator('.task-row').click();
  const detail = items.nth(1).locator('.task-detail');
  await expect(detail).toBeVisible();
  await expect(detail).toContainText('Started');
  await expect(detail).toContainText('Current task');
  await expect(detail.locator('.task-card-open')).toContainText(String(seed.cards[1]));
  await expect(detail.locator('.task-msgs')).toContainText('a message during the second task ' + nonce, { timeout: 10000 });
  // The first task ended when the second began, and did not receive that message.
  await items.nth(0).locator('.task-row').click();
  const first = items.nth(0).locator('.task-detail');
  await expect(first).toContainText('Ended');
  await expect(first.locator('.task-msgs')).not.toContainText('a message during the second task');
  // Clicking again closes it.
  await items.nth(0).locator('.task-row').click();
  await expect(first).toBeHidden();

  // The card opens on the board.
  await detail.locator('.task-card-open').first().click();
  await expect(page.locator('#tab-board')).toHaveClass(/active/);
  await expect(page.locator('body')).toContainText('Second task ' + nonce, { timeout: 10000 });
});

test("a worker's Tasks tab lists the model's task names, newest first, never its messages", async ({ page }) => {
  // Ethan, 2026-10-17: "the tasks tab is still wrong in the worker details
  // page. it shows messages it should be task names from the model."
  const nonce = Date.now().toString(36);
  const worker = 'e2e-probe';
  const message = 'an owner message that is not a task name ' + nonce;
  const names = ['Fix the tasks tab ' + nonce, 'Investigate the reminder ' + nonce];
  await page.route(`**/api/sessions/${worker}/task-history**`, route => route.fulfill({ json: {
    session: worker, measured: true, n_considered: 2, changes: [
      { to: names[0], from: names[1], card: 'AMUX-1', source: 'generated_title', ts: Date.now() / 1000 - 60 },
      { to: names[1], from: '', card: '', source: 'generated_title', ts: Date.now() / 1000 - 3600 },
    ] } }));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function', { timeout: 20000 });
  const status = await page.evaluate(async ({ worker, message }) => (await fetch('/api/history', { method: 'POST',
    headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text: message, type: 'user', session: worker }) })).status,
    { worker, message });
  expect(status).toBe(200);

  await page.evaluate((w) => (window as any).openPeek(w), worker);
  await expect(page.locator('#peek-overlay')).toBeVisible();
  await page.locator('#peek-tab-tasks').click();
  await expect(page.locator('#peek-tab-tasks')).toHaveClass(/active/);
  const panel = page.locator('#peek-tasks-panel');
  const items = panel.locator('.task-name-item');
  await expect(items).toHaveCount(2, { timeout: 15000 });
  await expect(items.nth(0)).toContainText(names[0]);
  await expect(items.nth(0)).toContainText('current');
  await expect(items.nth(0).locator('.msg-card-chip')).toHaveText('AMUX-1');
  await expect(items.nth(1)).toContainText(names[1]);
  await expect(panel).not.toContainText(message);
  await expect(page.locator('#peek-tasks-count')).toContainText('2 task names');
  const box = await items.nth(0).locator('.task-row').boundingBox();
  expect(box!.height, 'a 44px touch row').toBeGreaterThanOrEqual(44);
});

// Scheduler e2e findings (2026-10-11), driven through the real page and server.
// 1) A shell Run now answers 202 "result pending". The press tracker read that as
//    a command still in flight and, 30s later, toasted "Run now: still waiting on
//    the server; you can try again" over a run that had already finished. The
//    server now marks the START command applied; its receipt must settle.
// 2) A custom expression the server cannot parse surfaced only as a bare
//    "400: unparseable schedule_expr" toast. It is now marked on the field.
import {test, expect} from './fixtures';

async function boot(page) {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openSchedModal === 'function');
  await page.evaluate(() => (window as any).switchView('scheduler'));
}

test('a shell Run now settles its press as applied, so no stale "still waiting" toast', async ({page}) => {
  await boot(page);
  const id = await page.evaluate(async () => {
    const r = await fetch('/api/schedules', {method: 'POST', headers: (window as any)._authHeaders({'Content-Type': 'application/json'}),
      body: JSON.stringify({title: 'e2e run-now settle', kind: 'shell', command: 'true', schedule_expr: 'every 1h'})});
    return (await r.json()).id;
  });
  // The e2e server runs the scheduler in shadow mode (Run now answers 409), so
  // /run answers with the exact body a live shell run returns: shell_run_ack in
  // api/schedules.rs, pinned by its unit test. The page half runs for real.
  await page.route(new RegExp(`/api/schedules/${id}/run$`), r => r.fulfill({status: 202, json: {
    ok: true, pending: true, deduped: false, status: 'running', phase: 'applied',
    message: 'Run started on the host; its result appears in Runs', note: 'started on host; result pending',
    delivery: 'shell', submission: null, kind: 'shell', ran: 'e2e run-now settle', run_id: 1, ran_at: Math.floor(Date.now() / 1000), session: '', source: 'manual:e2e'}}));
  await page.evaluate(async sid => { await (window as any).fetchSchedules(); (window as any).renderScheduler(); (window as any).openSchedDetail(sid); }, id);
  const panel = page.locator('.sx-detail, [class*=sched-detail]').filter({hasText: id}).first();
  await panel.getByRole('button', {name: /Run now/}).click();
  await expect(page.locator('#toast')).toContainText(/run #\d+|Started on the host/i);
  await expect.poll(() => page.evaluate(() => {
    const recent = (0, eval)('_interactions').recent(50);
    const r = recent.reverse().find((x: any) => /\/api\/schedules\/[^/]+\/run$/.test(x.request?.path || ''));
    return r ? r.phase : 'none';
  })).toBe('applied');
});

test('an unparseable custom expression is marked on its field with forms that work', async ({page}) => {
  await boot(page);
  await page.evaluate(() => (window as any).openSchedModal());
  await page.locator('#sched-title').fill('e2e bad cron');
  await page.check('#sched-kind-shell');
  await page.locator('#sched-command').fill('true');
  await page.locator('.sx-preset[data-preset="custom"]').click();
  await page.locator('#sched-expr').fill('99 99 * * *');
  await page.locator('#sched-save-btn').click();
  await expect(page.locator('#sched-expr')).toHaveAttribute('aria-invalid', 'true');
  await expect(page.locator('#toast')).toContainText('Could not read "99 99 * * *"');
  await expect(page.locator('#toast')).toContainText('0 9 * * 1');
  await expect(page.locator('#sched-overlay')).toBeVisible();   // nothing saved, the form stays open
});

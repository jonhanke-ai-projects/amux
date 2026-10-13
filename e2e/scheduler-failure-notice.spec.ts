// A failing schedule shows who was told about it (Ethan, 2026-10-13: "if a
// scheduler is failing the worker should know and fix").
//
// The server records the notice on the schedule as failure_streak and
// failure_notice (runtime_jobs/schedule_failure.rs, unit-tested there). The
// e2e server runs the scheduler in shadow mode, so no run can fail here; the
// list response is the real one with those two fields set, exactly as the
// server publishes them. The page half runs for real on every project
// (desktop, 375px mobile, iOS Safari).
import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';

async function boot(page: Page): Promise<void> {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openSchedDetail === 'function'
    && typeof (window as any).fetchSchedules === 'function');
}

async function createSchedule(page: Page, title: string, session: string): Promise<string> {
  return page.evaluate(async ({ title, session }) => {
    const r = await fetch('/api/schedules', { method: 'POST', headers: (window as any)._authHeaders({ 'Content-Type': 'application/json' }),
      body: JSON.stringify({ title, session, kind: 'shell', command: 'exit 1', schedule_expr: 'daily at 09:00' }) });
    return (await r.json()).id as string;
  }, { title, session });
}

async function withNotice(page: Page, notices: Record<string, { streak: number; notice: Record<string, unknown> }>): Promise<void> {
  await page.route(/\/api\/schedules(\?.*)?$/, async (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    const res = await route.fetch();
    const rows = await res.json();
    for (const r of rows) {
      const n = notices[r.id];
      if (n) { r.failure_streak = n.streak; r.failure_notice = n.notice; }
    }
    await route.fulfill({ response: res, json: rows });
  });
}

test('a failing schedule row says which worker was told, or that it was routed to the owner', async ({ page }) => {
  await boot(page);
  const p = `sx-told-${Date.now()}-`;
  const told = await createSchedule(page, p + 'told', 'mixpeek-funnel');
  const routed = await createSchedule(page, p + 'routed', 'gone-worker');
  const quiet = await createSchedule(page, p + 'quiet', 'mixpeek-funnel');
  const at = Math.floor(Date.now() / 1000) - 2 * 3600;
  await withNotice(page, {
    [told]: { streak: 1, notice: { at, streak: 1, kind: 'first', route: 'worker', worker: 'mixpeek-funnel', signature: 'exit:1:x', error: '1 repair(s) needed', delivery: 'steer-1', suppressed: 0 } },
    [routed]: { streak: 2, notice: { at, streak: 1, kind: 'first', route: 'owner', worker: 'gone-worker', reason: 'archived', signature: 'refused::x', error: 'archived', delivery: 'AMUX-1', suppressed: 1 } },
    // A notice from a streak that has since passed must not show.
    [quiet]: { streak: 0, notice: { at, streak: 1, kind: 'first', route: 'worker', worker: 'mixpeek-funnel', signature: 's', error: 'e', suppressed: 0 } },
  });
  await page.evaluate(() => (window as any).switchView('scheduler'));
  await page.fill('#sched-search', p);
  await page.evaluate(async () => { await (window as any).fetchSchedules(); (window as any).renderScheduler(); });

  const row = (id: string) => page.locator(`.sx-row[data-sched-id="${id}"]`);
  await expect(row(told).locator('.sx-told')).toBeVisible();
  await expect(row(told).locator('.sx-told')).toHaveText('told mixpeek-funnel 2h ago');
  await expect(row(routed).locator('.sx-told')).toHaveText('routed to you: worker archived 2h ago');
  await expect(row(routed).locator('.sx-told')).toHaveAttribute('data-route', 'owner');
  await expect(row(quiet).locator('.sx-told')).toHaveCount(0);

  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow, 'page must not overflow horizontally').toBeLessThanOrEqual(1);

  // The detail view carries the same line.
  await page.evaluate((sid) => (window as any).openSchedDetail(sid), told);
  await expect(page.locator('.sx-told').filter({ hasText: 'told mixpeek-funnel' }).last()).toBeVisible();

  await page.evaluate(async (ids) => {
    for (const id of ids) await fetch('/api/schedules/' + id, { method: 'DELETE', headers: (window as any)._authHeaders({}) });
  }, [told, routed, quiet]);
});

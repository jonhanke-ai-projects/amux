// A worker's Schedules tab on a phone: with 2 active and 18 paused schedules the
// long Inactive group squeezed Daily Routines below its own header, so the
// active schedules could not be seen (Ethan, 2026-10-12, mixpeek-finances).
// Schedules are created on the e2e server for a stubbed worker.
import {test, expect} from './fixtures';

test('active schedules stay visible above a long Inactive group in the worker tab', async ({page}, info) => {
  const worker = {name: 'sched-tab-worker', running: true, status: 'idle', provider: 'claude', dir: '/tmp'};
  await page.route(/\/api\/sessions(?:\?.*)?$/, r => r.fulfill({json: [worker]}));
  await page.route(/\/api\/sessions\/sched-tab-worker\/peek/, r => r.fulfill({json: {name: worker.name, history: '', live: '❯ ', output: '❯ '}}));
  await page.setViewportSize({width: 375, height: 812});
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function');
  const tag = info.project.name + '-' + Date.now().toString(36);
  await page.evaluate(async ({w, tag}) => {
    const post = (body: any) => fetch('/api/schedules', {method: 'POST', headers: (window as any)._authHeaders({'Content-Type': 'application/json'}), body: JSON.stringify(body)});
    for (let i = 0; i < 2; i++) await post({title: `active ${i} ${tag}`, kind: 'shell', command: 'true', schedule_expr: 'every 1h', session: w});
    for (let i = 0; i < 18; i++) await post({title: `paused ${i} ${tag}`, kind: 'shell', command: 'true', schedule_expr: 'every 1h', session: w, enabled: 0});
  }, {w: worker.name, tag});
  await page.evaluate(w => { (0, eval)('sessions=' + JSON.stringify([w]) + '; render();'); (window as any).openPeek(w.name); }, worker);
  await page.evaluate(() => (window as any).setPeekTab('schedules'));
  const routine = page.locator('#peek-overlay .sx-group[data-group="routine"]');
  await expect(routine).toBeVisible();
  const rows = routine.locator('.sx-row').filter({hasText: tag});
  await expect(rows).toHaveCount(2);
  // The group is tall enough to show its rows, not clipped to its header.
  const m = await routine.evaluate(el => ({h: el.getBoundingClientRect().height, sh: el.scrollHeight}));
  expect(m.h, 'the group is not squeezed below its content').toBeGreaterThanOrEqual(m.sh - 1);
  await rows.first().scrollIntoViewIfNeeded();
  await expect(rows.first()).toBeInViewport();
});

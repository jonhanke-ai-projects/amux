import {test, expect} from './fixtures';

// Ethan, 2026-10-21, two screenshots of amux-homepage: "put the demos link in
// the header and footer too" sat in Steering as "1 queued" while the peek's
// footer said "Delivered while the worker was busy. Claude Code took it into
// the running turn". A queued send writes its history row when it is queued,
// so the row alone does not mean the worker has it. A message the worker's
// queue still holds is not listed as delivered; one taken mid-turn still is.
test('a message still in the queue is not shown as delivered while busy', async ({page}) => {
  const now = Date.now();
  const queued = 'put the demos link in the header and footer too';
  const taken = 'also check the mobile nav spacing';
  await page.route(/\/api\/sessions(?:\?.*)?$/, r => r.fulfill({json: [{
    name: 'busyw', running: true, status: 'active', dir: '/tmp',
    session_created: Math.floor(now / 1000) - 3600,
    steering: [{id: 7, transport_id: 'q-7', text: queued, queued_at: Math.floor(now / 1000) - 300}],
  }]}));
  await page.route(/\/api\/history(?:\?.*)?$/, r => r.fulfill({json: [
    {id: 3, session: 'busyw', type: 'user', kind: 'human', text: queued, ts: now - 300_000, delivery: 'queued'},
    {id: 2, session: 'busyw', type: 'user', kind: 'human', text: taken, ts: now - 400_000, delivery: 'direct'},
    {id: 1, session: 'busyw', type: 'user', kind: 'human', text: 'update the homepage CTAs', ts: now - 600_000, delivery: 'direct'},
  ]}));
  await page.route(/\/api\/sessions\/busyw\/peek\?/, r => r.fulfill({json: {
    name: 'busyw', history: '',
    live: '❯ update the homepage CTAs\n\n⏺ Working on the hero buttons\n\n✻ Tempering… (6m 8s)\n',
  }}));
  await page.route('**/api/sessions/busyw/subagents', r => r.fulfill({json: {session: 'busyw', subagents: []}}));

  await page.goto('/');
  await page.evaluate(() => (window as any).openPeek('busyw'));
  const banner = page.locator('.peek-unechoed');
  // The mid-turn message proves the banner renders at all in this setup.
  await expect(banner).toContainText(taken, {timeout: 15000});
  await expect(banner).not.toContainText(queued);
});

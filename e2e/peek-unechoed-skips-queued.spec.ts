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

// Ethan, 2026-10-22 11:47 (mixpeek-ops-server): the terminal printed the prompt
// "[11:46 AM] can you tell @mixpeek-cicd" while the recorded text wrapped the
// mention in backticks. The match on raw characters missed it, and a message
// that was printed was listed as taken without a prompt.
test('a printed prompt whose record differs only in markup is not listed as unprinted', async ({page}) => {
  const now = Date.now();
  const recorded = 'can you tell `@mixpeek-cicd`\n\n[amux: @-mentions above are amux workers, NOT files. Reach them via HTTP API, never tmux directly]';
  const unprinted = 'and ping the frontend lane about the nav';
  await page.route(/\/api\/sessions(?:\?.*)?$/, r => r.fulfill({json: [{
    name: 'opsw', running: true, status: 'active', dir: '/tmp', session_created: Math.floor(now / 1000) - 3600, steering: [],
  }]}));
  await page.route(/\/api\/history(?:\?.*)?$/, r => r.fulfill({json: [
    {id: 3, session: 'opsw', type: 'user', kind: 'human', text: unprinted, ts: now - 30_000, delivery: 'direct'},
    {id: 2, session: 'opsw', type: 'user', kind: 'human', text: recorded, ts: now - 60_000, delivery: 'direct'},
    {id: 1, session: 'opsw', type: 'user', kind: 'human', text: 'check the ops server health', ts: now - 600_000, delivery: 'direct'},
  ]}));
  await page.route(/\/api\/sessions\/opsw\/peek\?/, r => r.fulfill({json: {
    name: 'opsw', history: '',
    live: '❯ check the ops server health\n\n⏺ Done.\n\n❯ [11:46 AM] can you tell @mixpeek-cicd\n\n  [amux: @-mentions above are amux workers, NOT files. Reach them via HTTP API, never tmux directly]\n\n· Moseying… (29s)\n',
  }}));
  await page.route('**/api/sessions/opsw/subagents', r => r.fulfill({json: {session: 'opsw', subagents: []}}));
  await page.goto('/');
  await page.evaluate(() => (window as any).openPeek('opsw'));
  const banner = page.locator('.peek-unechoed');
  // Control: the message the terminal really does not show is still listed.
  await expect(banner).toContainText(unprinted, {timeout: 15000});
  await expect(banner).not.toContainText('can you tell');
});

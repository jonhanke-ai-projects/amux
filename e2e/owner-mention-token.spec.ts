// The owner's @-mention grant (api/mention_grants.rs) is honoured only with
// the dashboard's UI token. A composer send whose text has an @ carries it; a
// plain message does not; and the token is added at send time, never stored
// in the outbox. Sends are captured, never delivered.
import { test, expect } from './fixtures';

test('an @-message from the composer carries the UI token; a plain one does not', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  const sends: { text: string; token: string | null }[] = [];
  await page.route(/\/api\/sessions\/om-w\/send$/, async (route) => {
    const body = JSON.parse(route.request().postData() || '{}');
    sends.push({ text: body.text, token: route.request().headers()['x-amux-ui-token'] || null });
    await route.fulfill({ json: { ok: true, submitted: true, msg_id: body.msg_id } });
  });
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).doSend === 'function');
  const uiToken = await page.evaluate(() => (window as any)._AMUX_UI_TOKEN || '');
  await page.evaluate(() => { (globalThis as any).eval('sessions').push({ name: 'om-w', status: 'idle', running: true }); });

  await page.evaluate(() => (window as any).doSend('om-w', 'ask @amux for the deploy status'));
  await page.evaluate(() => (window as any).doSend('om-w', 'plain message, no mention'));
  await expect.poll(() => sends.length).toBe(2);
  const at = sends.find(s => s.text.includes('deploy status'))!;
  const plain = sends.find(s => s.text.includes('no mention'))!;
  if (uiToken) expect(at.token, 'the @-message carries the UI token').toBe(uiToken);
  else expect(at.token, 'no token on this host: nothing to carry').toBeNull();
  expect(plain.token, 'a plain message carries no token').toBeNull();

  // amux_app_html is the page's own cached copy, which already carries the
  // bootstrap token; the outbox and drafts must not gain one.
  const stored = await page.evaluate(() => JSON.stringify(Object.keys(localStorage)
    .filter(k => k !== 'amux_app_html').map(k => localStorage.getItem(k))));
  if (uiToken) expect(stored.includes(uiToken), 'the token is never written to the outbox or drafts').toBe(false);
});

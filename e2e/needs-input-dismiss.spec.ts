// The X on a card's "Waiting on you" box dismisses the ask for good, after an
// "are you sure" (Ethan, 2026-10-12). It used to only hide the box, so the card
// stayed in needsyou and kept flagging. Cancel in the confirm changes nothing.
// The worker, the card and every write are fakes.
import { test, expect, allowUnusedRoute } from './fixtures';

test('the X asks first, then tells the worker and takes the card out of needsyou', async ({ page }) => {
  const card: any = { id: 'NI-9002', title: 'Allow the debugging prompt', status: 'needsyou', session: 'nid-worker', desc: '', archived: false,
    ask_question: 'Can you click Allow so I can mint the key myself?' };
  const sent: any[] = [];
  const patches: any[] = [];
  const logged: any[] = [];
  await page.route('**/api/sessions/nid-worker/send', async r => { sent.push(JSON.parse(r.request().postData() || '{}')); await r.fulfill({ json: { ok: true, submitted: true } }); });
  await page.route('**/api/board/NI-9002**', async r => {
    if (r.request().method() === 'PATCH') {
      const p = JSON.parse(r.request().postData() || '{}');
      patches.push(p);
      if (p.desc_append) card.desc += '\n' + p.desc_append;
      if (p.status) card.status = p.status;
      return r.fulfill({ json: { ok: true, ...card } });
    }
    return r.fulfill({ json: card });
  });
  await page.route('**/api/needs-input/log', r => { logged.push(JSON.parse(r.request().postData() || '{}')); return r.fulfill({ json: { ok: true } }); });
  const worker = { name: 'nid-worker', running: true, status: 'idle', lifecycle: 'active', tags: [], dir: '/tmp', provider: 'claude' };
  await page.route('**/api/events**', r => r.abort());
  allowUnusedRoute(page, '**/api/events**');
  const sessionsList = /\/api\/sessions(?:\?.*)?$/;
  await page.route(sessionsList, r => r.request().method() === 'GET' ? r.fulfill({ json: [worker] }) : r.fallback());
  allowUnusedRoute(page, sessionsList);
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any)._bdOpenAnswer === 'function');
  await page.evaluate(({ card, worker }) => {
    const g = globalThis as any;
    if (!g.eval('sessions').some((x: any) => x.name === worker.name)) g.eval('sessions').push({ ...worker });
    g.eval('boardItems').push({ ...card });
    g._bdOpenAnswer(card.id, worker.name);
  }, { card, worker });
  const box = page.locator('#bd-answer');
  await expect(box).toBeVisible();
  const x = page.locator('#bd-answer-cancel');
  await expect(x).toHaveAttribute('aria-label', 'Dismiss this ask for good');

  // Cancel in the confirm: the box stays, nothing is sent or written.
  await x.click();
  const modal = page.locator('#modal-backdrop');
  await expect(modal).toHaveClass(/open/);
  await expect(page.locator('#modal-msg')).toContainText('Dismiss this ask for good?');
  await expect(page.locator('#modal-msg')).toContainText('nid-worker');
  await modal.getByRole('button', { name: 'Cancel' }).click();
  await expect(modal).not.toHaveClass(/open/);
  await expect(box).toBeVisible();
  await page.waitForTimeout(800);
  expect(sent).toHaveLength(0);
  expect(patches).toHaveLength(0);
  expect(card.status).toBe('needsyou');

  // Confirm: the worker is told, the card is noted and leaves needsyou.
  await x.click();
  await modal.getByRole('button', { name: 'Dismiss' }).click();
  await expect(page.locator('#toast')).toContainText('Dismissed', { timeout: 10000 });
  await expect.poll(() => sent.length, { timeout: 15000 }).toBeGreaterThanOrEqual(1);
  await page.waitForTimeout(1200);
  expect(sent).toHaveLength(1);
  expect(sent[0].text).toContain('Dismissed (NI-9002)');
  expect(sent[0].text).toContain('do not ask again');
  expect(patches.some(p => String(p.desc_append || '').includes('Dismissed without an answer by owner in the card'))).toBe(true);
  expect(card.status).toBe('todo');
  expect(logged.some(l => l.action === 'dismiss' && l.outcome === 'ok')).toBe(true);
  await expect(box).toBeHidden();
  // A re-render of the open card keeps it gone.
  await page.evaluate(c => (globalThis as any)._bdShowAnswer({ ...c }, false), card);
  await expect(box).toBeHidden();
});

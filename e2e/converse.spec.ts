// Converse tab, end to end (Ethan 2026-10-11: "chat with your worker ... hands
// free ... a log of the interactions ... replay any ... an expiration").
//
// WHAT IS REAL: the dashboard, the worker (a registered worker on this
// server), the Converse storage API (api/converse.rs: save, audio upload,
// list, read, delete, retention sweep), MediaRecorder recording a real
// MediaStream, and the replay player.
//
// WHAT IS STUBBED, and why: a headless browser cannot hear or speak, so
// SpeechRecognition, speechSynthesis and the microphone are stubbed in the
// page (the mic is a real oscillator MediaStream, so the recording has real
// bytes). The golden servers do not spawn workers from a throwaway home
// (AMUX-4724), so the worker's MODEL side is stubbed at its two routes: /send
// acknowledges and records what arrived, /last-message returns the reply. The
// dashboard still calls exactly those routes through doSend, which is the
// wiring under test.
import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { cleanup } from './teardown';

const BASELINE = 'An older reply from before this conversation started.';
const REPLY_1 = 'I am refactoring the board drain loop and the tests are green so far.';
const REPLY_2 = 'Second answer: the deploy finished ten minutes ago.';

async function installSpeechStubs(page: Page) {
  await page.addInitScript(() => {
    const w = window as any;
    w.__spoken = [];
    w.__cancelled = 0;
    class FakeRecognition {
      continuous = false; interimResults = false; lang = '';
      onresult: any = null; onend: any = null; onerror: any = null; started = false;
      start() { if (this.started) throw new Error('already started'); this.started = true; w.__rec = this; }
      stop() { this.abort(); }
      abort() { if (!this.started) return; this.started = false; setTimeout(() => this.onend && this.onend(), 0); }
    }
    w.SpeechRecognition = FakeRecognition;
    w.webkitSpeechRecognition = FakeRecognition;
    w.__say = (text: string) => {
      const r = w.__rec;
      if (!r || !r.started) return false;
      const res: any = [{ transcript: text, confidence: 0.95 }];
      res.isFinal = true;
      r.onresult({ resultIndex: 0, results: [res] });
      return true;
    };
    // Speech output: each utterance "plays" for 1.5s unless cancelled, like a
    // real engine, so talking over it can be observed.
    let current: any = null;
    const synth = {
      speaking: false, pending: false, paused: false, onvoiceschanged: null,
      getVoices: () => [],
      addEventListener() {}, removeEventListener() {},
      cancel() {
        w.__cancelled++;
        const u = current; current = null;
        if (u) { clearTimeout(u.__t); u.onend && u.onend({}); }
      },
      speak(u: any) {
        w.__spoken.push(u.text);
        current = u;
        u.__t = setTimeout(() => { if (current === u) current = null; u.onend && u.onend({}); }, 1500);
      },
    };
    Object.defineProperty(w, 'speechSynthesis', { value: synth, configurable: true });
    if (!w.SpeechSynthesisUtterance) w.SpeechSynthesisUtterance = class { text: string; constructor(t: string) { this.text = t; } };
    const fakeMic = async () => {
      const Ctx = w.AudioContext || w.webkitAudioContext;
      const ctx = new Ctx();
      const osc = ctx.createOscillator();
      osc.frequency.value = 220;
      const dest = ctx.createMediaStreamDestination();
      osc.connect(dest);
      osc.start();
      return dest.stream;
    };
    if (navigator.mediaDevices) {
      (navigator.mediaDevices as any).getUserMedia = fakeMic;
      (navigator.mediaDevices as any).enumerateDevices = async () => [];
    }
  });
}

test('Converse: talk to a worker hands-free, then replay, delete and expire the log', async ({ page, request }, testInfo) => {
  test.setTimeout(150_000);
  await installSpeechStubs(page);
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).openPeek === 'function' && typeof (window as any)._vxRender === 'function');
  const token = await page.evaluate(() => (window as any)._AMUX_AUTH_TOKEN as string);
  const uiToken = await page.evaluate(() => (window as any)._AMUX_UI_TOKEN as string);
  expect(token, 'served bootstrap must provide the API token').toBeTruthy();
  const auth = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' };
  const worker = `vx-e2e-${testInfo.project.name}-${Date.now()}`;
  const conv = `/api/converse/${worker}/conversations`;

  // The worker's model side (see header).
  // Answers with the server's acceptance shape ({ok, submitted}) so the
  // composer's delivery verification is satisfied, and counts by msg_id, the
  // way the server dedups a retried send.
  const sends: string[] = [];
  const seen = new Set<string>();
  let reply = BASELINE;
  await page.route(`**/api/sessions/${worker}/send`, async route => {
    const body = JSON.parse(route.request().postData() || '{}');
    if (!seen.has(body.msg_id)) {
      seen.add(body.msg_id);
      sends.push(String(body.text || ''));
      reply = sends.length === 1 ? REPLY_1 : REPLY_2;
    }
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ok: true, submitted: true, msg_id: body.msg_id }) });
  });
  await page.route(`**/api/sessions/${worker}/last-message`, route => route.fulfill({
    status: 200, contentType: 'application/json',
    body: JSON.stringify({ name: worker, text: reply, chars: reply.length, empty: false }),
  }));

  try {
    const created = await request.post('/api/sessions', { headers: auth, data: { name: worker, dir: '/tmp', start: false } });
    expect(created.status(), 'create the worker').toBe(201);

    // ── engines: PersonaPlex is reported unavailable, never faked ──────────
    const cfg = await (await request.get(`/api/converse/config?worker=${worker}`, { headers: auth })).json();
    expect(cfg.measured).toBe(true);
    expect(cfg.retention_days).toBe(30);
    const pp = cfg.engines.find((e: any) => e.id === 'personaplex');
    expect(pp.available, 'no PersonaPlex server is configured here').toBe(false);
    expect(pp.reason).toContain('NVIDIA GPU');

    // ── the tab exists and renders ───────────────────────────────────────
    await page.evaluate(async () => { await (window as any).fetchSessions?.(); });
    await page.evaluate(name => (window as any).openPeek(name), worker);
    const tab = page.locator('#peek-tab-converse');
    await expect(tab).toBeVisible();
    await expect(tab).toHaveText(/Converse/);
    await tab.click();
    const panel = page.locator('#peek-converse-panel');
    await expect(panel).toHaveClass(/active/);
    await expect(panel.locator('.vx-title')).toHaveText('Chat with your worker');
    await expect(panel.locator('#vx-retention-line')).toContainText('stored for 30 days');
    expect(await panel.locator('#vx-engine option[value="personaplex"]').evaluate(o => (o as HTMLOptionElement).disabled)).toBe(true);
    await expect(panel.locator('#vx-engine-note')).toContainText('NVIDIA GPU');

    // ── a hands-free conversation ────────────────────────────────────────
    await page.locator('#vx-call').click();
    await expect(page.locator('#vx-call-lbl')).toHaveText('End');
    await page.waitForFunction(() => (window as any).__rec?.started === true);
    // Give the baseline read a moment: the old reply must never be spoken.
    await page.waitForTimeout(400);
    expect(await page.evaluate(() => (window as any).__say('What are you working on right now?'))).toBe(true);
    await expect.poll(() => sends.length, { message: 'the utterance must reach the worker through /send', timeout: 10_000 }).toBe(1);
    expect(sends[0]).toContain('What are you working on right now?');
    await expect(page.locator('#vx-live')).toContainText(REPLY_1, { timeout: 15_000 });
    await expect.poll(() => page.evaluate(() => (window as any).__spoken as string[]), { timeout: 5_000 })
      .toContain(REPLY_1);
    // Interrupt anytime: talking over the worker cancels its speech.
    const cancelledBefore = await page.evaluate(() => (window as any).__cancelled as number);
    expect(await page.evaluate(() => (window as any).__say('Never mind, has the deploy finished?')), 'recognition is still listening').toBe(true);
    await expect.poll(() => page.evaluate(() => (window as any).__cancelled as number)).toBeGreaterThan(cancelledBefore);
    await expect.poll(() => sends.length, { timeout: 10_000, message: 'sends: ' + JSON.stringify(sends) }).toBe(2);
    await expect(page.locator('#vx-live')).toContainText(REPLY_2, { timeout: 15_000 });
    const spoken: string[] = await page.evaluate(() => (window as any).__spoken);
    expect(spoken, 'the reply from before the conversation is never spoken').not.toContain(BASELINE);
    await page.waitForTimeout(1200);   // let a little audio accumulate
    await page.locator('#vx-call').click();
    await expect(page.locator('#vx-call-lbl')).toHaveText('Start');

    // ── it is in the history, with duration and expiry ───────────────────
    const row = panel.locator('.vx-row').first();
    await expect(row).toContainText('What are you working on right now?', { timeout: 15_000 });
    await expect(row.locator('.vx-row-meta')).toContainText(/\d+s/);
    await expect(row.locator('.vx-row-exp')).toContainText('Expires in 30 days');
    const listed = await (await request.get(conv, { headers: auth })).json();
    expect(listed.measured).toBe(true);
    expect(listed.n).toBe(1);
    const rec = listed.conversations[0];
    const full = await (await request.get(`${conv}/${rec.id}`, { headers: auth })).json();
    const roles = full.turns.map((t: any) => t.role);
    expect(roles.filter((r: string) => r === 'user'), 'both utterances logged').toHaveLength(2);
    expect(roles.filter((r: string) => r === 'worker'), 'both replies logged').toHaveLength(2);
    expect(full.turns.find((t: any) => t.role === 'worker').text).toBe(REPLY_1);
    expect(full.turns.every((t: any, i: number) => i === 0 || t.t_ms >= full.turns[i - 1].t_ms), 'turn offsets are ordered').toBe(true);
    expect(sends[1]).toContain('has the deploy finished?');
    expect(rec.duration_ms).toBeGreaterThan(1000);
    // Chromium records the stubbed mic. Playwright's WebKit build exposes
    // MediaRecorder but emits no data for a WebAudio stream, so there the
    // conversation is transcript-only and replay speaks it (asserted below);
    // the client reports that as converse_recording_empty.
    const canRecord = testInfo.project.name !== 'ios-safari'
      && await page.evaluate(() => typeof (window as any).MediaRecorder === 'function');
    if (!rec.audio) expect(canRecord, 'audio missing on a browser that records').toBe(false);
    if (canRecord) {
      expect(rec.audio, 'MediaRecorder audio is uploaded with the conversation').toBeTruthy();
      expect(rec.audio.bytes).toBeGreaterThan(0);
      const audio = await request.get(`${conv}/${rec.id}/audio`, { headers: auth });
      expect(audio.status()).toBe(200);
      expect((await audio.body()).length).toBe(rec.audio.bytes);
    }
    testInfo.annotations.push({ type: 'audio', description: canRecord ? `recorded ${rec.audio?.bytes} bytes` : 'no MediaRecorder: transcript-only replay' });

    // ── replay ──────────────────────────────────────────────────────────
    const player = panel.locator('#vx-player');
    await expect(player.locator('.vx-ptitle')).toContainText('What are you working on');
    await expect(player.locator('.vx-pexp')).toHaveText('Expires in 30 days');
    await expect(player.locator('.vx-pturn')).toHaveCount(full.turns.length);
    const workerTurn = player.locator('.vx-pturn', { hasText: REPLY_1 });
    await expect(workerTurn).toHaveCount(1);
    const spokenBefore = (await page.evaluate(() => (window as any).__spoken as string[])).length;
    await page.locator('#vx-pplay').click();
    await expect(page.locator('#vx-pplay')).toHaveAttribute('aria-label', 'Pause');
    // Jump to the worker's first turn: replay re-speaks it (browser speech is
    // not in the recording), and the transcript highlights it.
    await workerTurn.click();
    await expect.poll(async () => (await page.evaluate(() => (window as any).__spoken as string[])).slice(spokenBefore),
      { message: 'replay speaks the worker turn', timeout: 10_000 }).toContain(REPLY_1);
    await expect.poll(() => page.locator('#vx-pos').textContent(), { timeout: 10_000 }).not.toBe('0:00');
    await page.locator('#vx-speed').click();
    await expect(page.locator('#vx-speed')).toContainText('1.25');
    await page.locator('#vx-pplay').click();
    await expect(page.locator('#vx-pplay')).toHaveAttribute('aria-label', 'Play');
    await player.getByRole('button', { name: 'Details' }).click();
    await expect(player.locator('.vx-pbody')).toContainText('Engine');

    // ── layout: fits the screen, touch targets are touch-sized ──────────
    const geo = await page.evaluate(() => {
      const p = document.getElementById('peek-converse-panel')!;
      const box = (s: string) => { const r = document.querySelector(s)!.getBoundingClientRect(); return { w: r.width, h: r.height }; };
      return { overflow: p.scrollWidth - p.clientWidth, call: box('#vx-call'), play: box('.vx-row-play'), mute: box('#vx-mute') };
    });
    expect(geo.overflow, 'no horizontal overflow in the Converse panel').toBeLessThanOrEqual(1);
    for (const k of ['call', 'play', 'mute'] as const) {
      expect(geo[k].w, `${k} width`).toBeGreaterThanOrEqual(44);
      expect(geo[k].h, `${k} height`).toBeGreaterThanOrEqual(44);
    }
    await testInfo.attach(`converse-${testInfo.project.name}`, { body: await page.screenshot(), contentType: 'image/png' });

    // ── delete from the row menu (two taps) ─────────────────────────────
    await row.locator('.vx-row-more').click();
    await row.getByRole('menuitem', { name: 'Delete' }).click();
    await row.getByRole('menuitem', { name: 'Tap again to delete' }).click();
    await expect(panel.locator('.vx-row')).toHaveCount(0, { timeout: 10_000 });
    expect((await request.get(`${conv}/${rec.id}`, { headers: auth })).status()).toBe(404);

    // ── expiry: an old conversation is hidden, then swept ───────────────
    const oldId = `vx-old-${Date.now()}`;
    const old = await request.post(conv, { headers: auth, data: {
      id: oldId, title: 'Old chat', engine: 'browser', started_at: Date.now() - 40 * 86_400_000,
      turns: [{ role: 'user', text: 'hello from last month', t_ms: 0 }],
    } });
    expect(old.status()).toBe(200);
    const hidden = await (await request.get(conv, { headers: auth })).json();
    expect(hidden.conversations.map((c: any) => c.id)).not.toContain(oldId);
    expect(hidden.expired_pending_sweep).toBe(1);
    expect((await request.get(`${conv}/${oldId}`, { headers: auth })).status()).toBe(410);
    const swept = await (await request.post('/api/converse/sweep', { headers: auth })).json();
    expect(swept.measured).toBe(true);
    expect(swept.removed).toBeGreaterThanOrEqual(1);
    expect(swept.verdict).toBe('removed');
    const after = await (await request.get(conv, { headers: auth })).json();
    expect(after.expired_pending_sweep).toBe(0);
    expect(after.n_considered).toBe(0);
    expect((await request.get(`${conv}/${oldId}`, { headers: auth })).status()).toBe(404);
  } finally {
    await cleanup('close peek', () => page.evaluate(() => (window as any).closePeek?.()));
    await cleanup('delete fixture worker', () => request.post(`/api/sessions/${encodeURIComponent(worker)}/delete`, {
      headers: { ...auth, 'X-Amux-UI-Token': uiToken }, data: {},
    }));
  }
});

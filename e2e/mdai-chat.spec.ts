// MDAI chat (Ethan, 2026-10-17: "in mdai full view add the ability to talk to it
// and add to it via chat ... I want to be able to say 'look at the messages
// responsible for lifecycle work, and include that in the table'").
//
// Two tests, and which parts are real is stated per test:
//
// 1. The page, against a stubbed chat endpoint: the composer, the exchange, the
//    changed-lines view, the run that follows, undo, a refused edit, and the
//    phone layout. Only /api/files/mdai/chat and /run are stubbed; the doc API
//    is the real server's.
// 2. The whole path on a real server: POST /chat calls the server's model
//    client, which the `mdai_api_base` pref points at a loopback stand-in for
//    the Messages API (as mdai-pages.spec.ts does), the server validates and
//    writes the file, and the run that follows sends the NEW instruction and
//    the NEW source to the model. This needs a server built with the chat
//    route; on one without it the test is skipped and says why.
import { createServer, type IncomingMessage, type Server } from 'node:http';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { APIRequestContext, Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { cleanup } from './teardown';

const NODE = '---\ntitle: Workstream Resumption\nmodel: sonnet\nrefresh: manual\nsources:\n- path: amux:messages?days=all&limit=4000&kb=300\n  prompt: My Amux message history.\n---\n# Workstream Resumption\n\nBuild a table of my workstreams with their last known state.\n';
const ASK = 'look at the messages responsible for lifecycle work, and include that in the table';
const NEW_BODY = '# Workstream Resumption\n\nBuild a table of my workstreams with their last known state.\nAdd a Lifecycle column: for each workstream, summarize the lifecycle work found in the lifecycle messages.\n';
const NEW_SOURCE = 'amux:messages?days=all&q=lifecycle';

async function setup(page: Page, request: APIRequestContext, prefix: string) {
  const dir = await mkdtemp(join(tmpdir(), prefix));
  const file = join(dir, 'Workstream Resumption.mdai');
  await writeFile(file, NODE);
  await page.goto('/');
  const headers = await page.evaluate(() => ({ Authorization: 'Bearer ' + (window as any)._AMUX_AUTH_TOKEN }));
  const pref = (key: string, value: string) => request.post('/api/prefs', { headers, data: { key, value } });
  const prior = await Promise.all(['mdai_root', 'mdai_api_base'].map(async k => (await (await request.get('/api/prefs?key=' + k, { headers })).json()).value || ''));
  expect((await pref('mdai_root', dir)).ok()).toBeTruthy();
  return { dir, file, headers, pref, prior };
}

async function openNode(page: Page, file: string) {
  await page.reload();
  await page.waitForFunction(() => typeof (window as any).openMdaiNode === 'function');
  await page.evaluate(p => (window as any).openMdaiNode(p, { autorun: false }), file);
  await expect(page.locator('#mdai-overlay')).toHaveClass(/active/);
  await expect(page.locator('#mdai-chatbar')).toBeVisible();
}

test('MDAI chat: the page shows the exchange and the changed lines, runs again, undoes, and says why an edit was refused', async ({ page, request }, info) => {
  test.setTimeout(120_000);
  const narrow = page.viewportSize()!.width <= 600;
  const s = await setup(page, request, 'amux-mdai-chat-ui-');
  page.on('dialog', d => d.accept());
  const posts: any[] = [];
  let runs = 0;
  let refuse = false;
  try {
    await page.route('**/api/files/mdai/chat?**', r => r.fulfill({ json: { path: 'Workstream Resumption.mdai', turns: [], unreadable: 0, measured: true, n_considered: 0 } }));
    await page.route('**/api/files/mdai/run', r => { runs++; return r.fulfill({ status: 503, json: { error: 'no model in this test' } }); });
    await page.route('**/api/files/mdai/chat', async r => {
      const body = r.request().postDataJSON();
      posts.push(body);
      if (refuse) {
        return r.fulfill({ status: 422, json: { error: 'The model proposed an edit the file cannot hold (sources: unknown amux source). Nothing was changed.', verdict: 'mdai_chat_edit_invalid',
          turn: { ts: Date.now(), message: body.message, error: 'The model proposed an edit the file cannot hold (sources: unknown amux source). Nothing was changed.', changed: false } } });
      }
      // Write the file the way the server would, so the page reads a real doc back.
      const after = NODE.replace('  prompt: My Amux message history.\n', '  prompt: My Amux message history.\n- path: ' + NEW_SOURCE + '\n  prompt: Messages about lifecycle work.\n')
        .replace(/# Workstream Resumption\n\n[\s\S]*$/, NEW_BODY);
      await writeFile(s.file, after);
      const doc = await (await request.get('/api/files/mdai/doc?path=' + encodeURIComponent(s.file), { headers: s.headers })).json();
      return r.fulfill({ json: { ok: true, changed: true, doc,
        turn: { ts: Date.now(), message: body.message, reply: 'Added a Lifecycle column and a source for lifecycle messages.', changed: true, body_changed: true, sources_changed: true,
          before: NODE, after, truncated: false, model: 'sonnet', dur_ms: 4200, error: null } } });
    });
    await openNode(page, s.file);

    // The composer: 44px targets, no sideways overflow on a phone.
    const ta = page.locator('#mdai-chat-input');
    const send = page.locator('#mdai-chat-send');
    expect((await send.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    expect((await ta.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
    if (narrow) expect(await page.locator('#mdai-overlay .mdai-tabs').evaluate(e => e.scrollWidth - e.clientWidth), 'six tabs still fit').toBeLessThanOrEqual(1);

    // Send the owner's exact request.
    await ta.fill(ASK);
    await send.click();
    await expect(page.locator('#mdai-overlay .mdai-tab[data-tab=chat]')).toHaveClass(/active/);
    await expect(page.locator('#mdai-chat-log .mdai-msg.me .mdai-bubble').first()).toHaveText(ASK);
    await expect(page.locator('#mdai-chat-log .mdai-msg.ai .mdai-bubble').first()).toContainText('Added a Lifecycle column');
    expect(posts[0].message).toBe(ASK);
    expect(typeof posts[0].base_mtime_ms, 'the edit is pinned to the version on screen').toBe('number');
    await expect(ta, 'the box clears after a send').toHaveValue('');
    // The changed lines: the new source and the new instruction line are added.
    const diff = page.locator('#mdai-chat-log .mdai-diff');
    await expect(diff).toBeVisible();
    await expect(diff.locator('.mdai-diff-l.add', { hasText: NEW_SOURCE })).toHaveCount(1);
    await expect(diff.locator('.mdai-diff-l.add', { hasText: 'Add a Lifecycle column' })).toHaveCount(1);
    await expect(page.locator('#mdai-chat-log .mdai-msg.ai').first()).toContainText('changed instruction and sources');
    // The page holds the new file: the Sources tab count and the editor.
    await expect(page.locator('#mdai-overlay .mdai-tab[data-tab=sources] .mdai-tab-n')).toHaveText('2');
    await expect(page.locator('#mdai-overlay .mdai-tab[data-tab=chat] .mdai-tab-n')).toHaveText('1');
    // A change starts a run through the ordinary run flow.
    await expect.poll(() => runs, { message: 'the node runs after the edit' }).toBe(1);
    await expect(page.locator('#mdai-body .mdai-err')).toContainText('no model in this test');
    if (narrow) for (const b of await page.locator('#mdai-chat-log .mdai-chat-acts .btn').all()) expect((await b.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
    await page.screenshot({ path: info.outputPath('mdai-chat-applied.png') });

    // Undo puts the file back through the doc API.
    await page.locator('#mdai-chat-log .mdai-chat-acts .btn', { hasText: 'Undo' }).click();
    await expect.poll(() => readFile(s.file, 'utf8')).toBe(NODE);
    await expect(page.locator('#mdai-chat-log .mdai-pill.ok')).toHaveText('undone');
    await expect(page.locator('#mdai-overlay .mdai-tab[data-tab=sources] .mdai-tab-n')).toHaveText('1');

    // A refused edit is shown in the conversation, keeps the typed text, and does not run.
    refuse = true;
    await ta.fill('add the board as a source');
    await ta.press('Enter');
    await expect(page.locator('#mdai-chat-log .mdai-bubble.bad')).toContainText('cannot hold');
    await expect(ta, 'a failed message can be sent again').toHaveValue('add the board as a source');
    expect(runs, 'a refused edit does not start a run').toBe(1);
    expect(await readFile(s.file, 'utf8')).toBe(NODE);
    await page.screenshot({ path: info.outputPath('mdai-chat-refused.png') });
  } finally {
    await cleanup('restore MDAI prefs', () => Promise.all([s.pref('mdai_root', s.prior[0]), s.pref('mdai_api_base', s.prior[1])]), info);
    await cleanup('remove temp folder', () => rm(s.dir, { recursive: true, force: true }), info);
  }
});

// A loopback stand-in for the Messages API. A chat-edit prompt gets a tagged
// edit; a node run gets a report that echoes what it was asked, so the test can
// see the run used the edited file.
async function fakeMessagesApi() {
  const chatPrompts: string[] = [];
  const runPrompts: string[] = [];
  const server: Server = createServer(async (req: IncomingMessage, res) => {
    let body = '';
    for await (const chunk of req) body += chunk;
    if (req.method !== 'POST' || !req.url?.endsWith('/v1/messages')) { res.writeHead(404).end(); return; }
    const prompt = String(JSON.parse(body).messages?.[0]?.content || '');
    let text: string;
    if (prompt.includes('You edit an .mdai file') && prompt.includes('Workstream Resumption')) {
      chatPrompts.push(prompt);
      text = '<reply>Added a Lifecycle column and a source that selects the lifecycle messages.</reply>\n<body>\n' + NEW_BODY + '</body>\n<sources>'
        + JSON.stringify([{ path: 'amux:messages?days=all&limit=4000&kb=300', prompt: 'My Amux message history.' }, { path: NEW_SOURCE, prompt: 'Messages about lifecycle work.' }]) + '</sources>';
    } else if (prompt.includes('Build a table of my workstreams')) {
      runPrompts.push(prompt);
      text = prompt.includes('Lifecycle column') ? '| Workstream | State | Lifecycle |\n|---|---|---|\n| Workers | live | restart path fixed |' : '| Workstream | State |\n|---|---|\n| Workers | live |';
    } else {
      res.writeHead(401, { 'content-type': 'application/json' }).end(JSON.stringify({ type: 'error', error: { type: 'authentication_error', message: 'stand-in: not this spec\'s prompt' } }));
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ content: [{ type: 'text', text }], usage: { input_tokens: 100, output_tokens: 50 } }));
  });
  await new Promise<void>(r => server.listen(0, '127.0.0.1', () => r()));
  const port = (server.address() as { port: number }).port;
  return { url: `http://127.0.0.1:${port}`, chatPrompts, runPrompts, close: () => new Promise<void>(r => { server.closeAllConnections(); server.close(() => r()); }) };
}

test('MDAI chat: a real server edits the file through the model and the next run uses it', async ({ page, request }, info) => {
  test.setTimeout(180_000);
  const s = await setup(page, request, 'amux-mdai-chat-real-');
  const api = await fakeMessagesApi();
  try {
    const probe = await request.get('/api/files/mdai/chat?path=' + encodeURIComponent(s.file), { headers: s.headers });
    test.skip(probe.status() === 404 || probe.status() === 405, 'this server was built without POST /api/files/mdai/chat (run with AMUX_E2E_WORKING_TREE=1 to test uncommitted server code)');
    expect(probe.ok()).toBeTruthy();
    expect((await s.pref('mdai_api_base', api.url)).ok()).toBeTruthy();
    await openNode(page, s.file);
    await page.locator('#mdai-chat-input').fill(ASK);
    await page.locator('#mdai-chat-send').click();
    await expect(page.locator('#mdai-chat-log .mdai-msg.ai .mdai-bubble').first()).toContainText('Added a Lifecycle column', { timeout: 60_000 });

    // The model saw the file and the request, and the server wrote a file the engine reads.
    expect(api.chatPrompts.length).toBe(1);
    expect(api.chatPrompts[0]).toContain(ASK);
    expect(api.chatPrompts[0]).toContain('Build a table of my workstreams');
    const text = await readFile(s.file, 'utf8');
    expect(text).toContain(NEW_SOURCE);
    expect(text).toContain('Add a Lifecycle column');
    expect(text, 'fields the edit did not touch survive').toContain('refresh: manual');
    await expect(page.locator('#mdai-chat-log .mdai-diff .mdai-diff-l.add', { hasText: NEW_SOURCE })).toHaveCount(1);

    // The run that follows sends the NEW instruction, and its output has the new column.
    await expect.poll(() => api.runPrompts.length, { timeout: 60_000 }).toBeGreaterThanOrEqual(1);
    expect(api.runPrompts[api.runPrompts.length - 1]).toContain('Lifecycle column');
    await page.locator('#mdai-chat-log .mdai-chat-acts .btn', { hasText: 'See output' }).click();
    await expect(page.locator('#mdai-body .mdai-output')).toContainText('restart path fixed', { timeout: 60_000 });

    // The conversation is kept: a reload reads it back from the server.
    await openNode(page, s.file);
    await page.locator('#mdai-overlay .mdai-tab[data-tab=chat]').click();
    await expect(page.locator('#mdai-chat-log .mdai-msg.me .mdai-bubble').first()).toHaveText(ASK);
    await page.screenshot({ path: info.outputPath('mdai-chat-real.png') });
  } finally {
    await cleanup('restore MDAI prefs', () => Promise.all([s.pref('mdai_root', s.prior[0]), s.pref('mdai_api_base', s.prior[1])]), info);
    await cleanup('stop the stand-in Messages API', () => api.close(), info);
    await cleanup('remove temp folder', () => rm(s.dir, { recursive: true, force: true }), info);
  }
});

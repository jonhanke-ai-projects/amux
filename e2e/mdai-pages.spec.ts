// MDAI pages, end to end (Ethan, 2026-10-14: "re-create the mdai pages (list,
// details, create, edit) ... the ability to create this mdai and see it via
// browser", and "it also needs to be viewable/editable entirely in the ui").
//
// Everything below goes through the dashboard's own controls. The server is
// real: the doc API writes real .mdai files, runs are real detached runs with
// the real single-flight registry, history rows are real. ONE thing is stubbed,
// at the server's provider boundary and disclosed here: the model. The e2e home
// carries a non-secret API key, so the server's ApiModel answers each node, and
// the `mdai_api_base` pref points it at a loopback stand-in for the Messages API
// started by this spec. That keeps CI deterministic and free; the same journey
// was run once against a real model locally (see the PR).
import { createServer, type IncomingMessage, type Server } from 'node:http';
import { mkdtemp, readFile, rm, writeFile, mkdir, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { cleanup } from './teardown';

type Mode = { delayMs: number; fail?: string };

async function fakeMessagesApi() {
  const mode: Mode = { delayMs: 0 };
  const prompts: string[] = [];
  const server: Server = createServer(async (req: IncomingMessage, res) => {
    let body = '';
    for await (const chunk of req) body += chunk;
    if (req.method !== 'POST' || !req.url?.endsWith('/v1/messages')) { res.writeHead(404).end(); return; }
    const prompt = JSON.parse(body).messages?.[0]?.content || '';
    prompts.push(prompt);
    const { delayMs, fail } = mode;
    setTimeout(() => {
      if (res.destroyed) return;
      if (fail) {
        res.writeHead(529, { 'content-type': 'application/json' }).end(JSON.stringify({ type: 'error', error: { type: 'overloaded_error', message: fail } }));
        return;
      }
      const text = '# Where I Left Off\n\n| Workstream | Last state | Next action |\n|---|---|---|\n| MDAI pages | rework in review | run the e2e |\n\n## Workstream registry\n\n- **MDAI pages**: ' + prompts.length + ' run(s) seen.';
      res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({
        content: [{ type: 'text', text }], usage: { input_tokens: 1234, output_tokens: 567 },
      }));
    }, delayMs);
  });
  await new Promise<void>(r => server.listen(0, '127.0.0.1', () => r()));
  const port = (server.address() as { port: number }).port;
  return { url: `http://127.0.0.1:${port}`, mode, prompts, close: () => new Promise<void>(r => { server.closeAllConnections(); server.close(() => r()); }) };
}

const PROMPT = '# Amux Workstream Reconstruction & Resumption\n\nAnalyze my complete Amux message history and reconstruct all the workstreams.\n\n## Final instruction\n\nStart with a **"Where I Left Off" dashboard**.\n';

const tab = (page: Page, t: string) => page.locator(`#mdai-overlay .mdai-tab[data-tab=${t}]`).click();
// An unchanged node is served from its cache with no model call, which is the
// engine working. A run that must reach the model changes the instruction
// first; Run saves the unsaved edit before it starts.
let edits = 0;
async function editInstruction(page: Page) {
  await tab(page, 'edit');
  await page.locator('#mdai-instr-ta').fill(PROMPT + '\nKeep it short.\nEdit ' + (++edits) + '.\n');
}
const fileText = (p: string) => readFile(p, 'utf8');

test('MDAI: create and configure a node entirely in the UI, every field round-trips, runs are single-flight, honest and recoverable', async ({ page, request }, info) => {
  test.setTimeout(300_000);
  const narrow = page.viewportSize()!.width <= 600;
  const api = await fakeMessagesApi();
  const dir = await mkdtemp(join(tmpdir(), 'amux-mdai-pages-'));
  await mkdir(join(dir, 'team'), { recursive: true });
  await writeFile(join(dir, 'team', 'notes.md'), 'Ship the MDAI pages.\n');
  page.on('dialog', d => d.accept());
  await page.goto('/');
  const headers = await page.evaluate(() => ({ Authorization: 'Bearer ' + (window as any)._AMUX_AUTH_TOKEN }));
  const pref = (key: string, value: string) => request.post('/api/prefs', { headers, data: { key, value } });
  const prior = await Promise.all(['mdai_root', 'mdai_api_base'].map(async k => (await (await request.get('/api/prefs?key=' + k, { headers })).json()).value || ''));
  try {
    expect((await pref('mdai_root', dir)).ok()).toBeTruthy();
    expect((await pref('mdai_api_base', api.url)).ok()).toBeTruthy();
    await page.reload();
    await page.waitForFunction(() => typeof (window as any).openMdaiNode === 'function');
    await page.evaluate(() => (window as any).switchView('mdai'));

    // ---- List: empty state, then the create page --------------------------
    await expect(page.locator('#mdai-list')).toContainText('No MDAI nodes yet');
    await page.locator('#mdai-new-btn').click();
    await expect(page.locator('#mdai-create-page')).toBeVisible();
    // An empty instruction is refused before anything is written.
    await page.locator('#mdai-new-name').fill('Workstreams');
    await page.locator('#mdai-create-page .mdai-create-go:not(.primary)').click();
    await expect(page.locator('#mdai-new-error')).toContainText('Write the instruction');

    // ---- Create: every field through a control ------------------------------
    await page.locator('#mdai-new-folder').fill('team');
    await page.locator('#mdai-new-title').fill('Where I left off');
    await page.getByRole('button', { name: '+ Amux message history' }).click();
    await page.locator('#new-days-0').selectOption('all');
    await page.locator('#new-limit-0').fill('300');
    await page.locator('#new-offset-0').fill('10');
    await page.locator('#new-kb-0').fill('200');
    await page.locator('#new-prompt-0').fill('Owner messages, oldest first.');
    await page.getByRole('button', { name: '+ File or folder' }).click();
    await page.locator('#new-path-1').fill('notes.md');
    await page.locator('#mdai-new-model').selectOption('__custom');
    await page.locator('#mdai-new-model-custom').fill('claude-sonnet-4-6');
    await page.locator('#mdai-new-timeout').fill('600');
    await page.locator('#mdai-new-body').fill(PROMPT);
    await page.locator('#mdai-create-page .mdai-seg button[data-mode=preview]').click();
    await expect(page.locator('#mdai-new-preview')).toContainText('Where I Left Off');
    if (narrow) {
      for (const sel of ['#mdai-create-page .mdai-seg button', '#mdai-create-page .mdai-create-go', '#new-sources .mdai-add-row .btn']) {
        for (const b of await page.locator(sel).all()) expect((await b.boundingBox())!.height, sel).toBeGreaterThanOrEqual(44);
      }
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
    await page.screenshot({ path: info.outputPath('mdai-create.png'), fullPage: true });
    await page.locator('#mdai-create-page .mdai-create-go:not(.primary)').click();
    await expect(page.locator('#mdai-overlay')).toHaveClass(/active/);
    await expect(page.locator('#mdai-title')).toHaveText('Workstreams.mdai');

    // The file on disk holds exactly what the form said.
    const file = join(dir, 'team', 'Workstreams.mdai');
    let text = await fileText(file);
    for (const want of ['title: Where I left off', 'model: claude-sonnet-4-6', 'refresh: manual', 'timeout: 600',
      'amux:messages?days=all&limit=300&offset=10&kb=200', 'Owner messages, oldest first.', 'notes.md', '"Where I Left Off" dashboard']) {
      expect(text, want).toContain(want);
    }
    expect(api.prompts.length, 'creating does not run the node').toBe(0);

    // ---- Reload: every field reads back into its control ---------------------
    await page.reload();
    await page.evaluate(() => (window as any).switchView('mdai'));
    await expect(page.locator('.mdai-tr[data-path]')).toHaveCount(1);
    await expect(page.locator('.mdai-tr[data-path]')).toContainText('Where I left off');
    await page.locator('.mdai-tr[data-path]').click();
    await tab(page, 'settings');
    await expect(page.locator('#mdai-set-title')).toHaveValue('Where I left off');
    await expect(page.locator('#mdai-set-model')).toHaveValue('__custom');
    await expect(page.locator('#mdai-set-model-custom')).toHaveValue('claude-sonnet-4-6');
    await expect(page.locator('#mdai-set-timeout')).toHaveValue('600');
    await expect(page.locator('#mdai-set-refresh')).not.toBeChecked();
    await tab(page, 'sources');
    await expect(page.locator('#src-days-0')).toHaveValue('all');
    await expect(page.locator('#src-limit-0')).toHaveValue('300');
    await expect(page.locator('#src-offset-0')).toHaveValue('10');
    await expect(page.locator('#src-kb-0')).toHaveValue('200');
    await expect(page.locator('#src-prompt-0')).toHaveValue('Owner messages, oldest first.');
    await expect(page.locator('#src-path-1')).toHaveValue('notes.md');
    await tab(page, 'edit');
    await expect(page.locator('#mdai-instr-ta')).toHaveValue(PROMPT);

    // ---- Edit: settings, sources, the API call and the instruction ---------------
    await tab(page, 'settings');
    await page.locator('#mdai-set-title').fill('Resume here');
    await page.locator('#mdai-set-model').selectOption('haiku');
    await page.locator('#mdai-set-timeout').fill('5');
    await page.locator('#mdai-set-save').click();
    await expect(page.locator('#mdai-body .mdai-field-err')).toContainText('between 30 and 3600');
    await page.locator('#mdai-set-timeout').fill('120');
    await page.locator('#mdai-set-refresh').check();
    await page.locator('#mdai-set-save').click();
    await expect.poll(() => fileText(file)).toContain('title: Resume here');
    text = await fileText(file);
    expect(text).toContain('model: haiku');
    expect(text).toContain('timeout: 120');
    expect(text).toContain('refresh: open');
    await tab(page, 'sources');
    await page.locator('#src-limit-0').fill('150');
    await page.locator('#src-days-0').selectOption('30');
    await page.locator('.mdai-src[data-kind=path] button', { hasText: 'Remove' }).click();
    await page.locator('#mdai-body details.mdai-fetch summary').click();
    await page.locator('#mdai-fetch-url').fill(api.url + '/data');
    await page.locator('#mdai-fetch-headers').fill('Accept: application/json');
    await page.locator('#mdai-fetch-select').fill('data.items');
    await page.locator('#mdai-src-save').click();
    await expect.poll(() => fileText(file)).toContain('select: data.items');
    text = await fileText(file);
    expect(text).toContain('amux:messages?days=30&limit=150&offset=10&kb=200');
    expect(text).not.toContain('notes.md');
    expect(text).toContain('Accept: application/json');
    // Clearing the URL removes the API call again. The edit starts once the
    // save has finished rendering; a render that lands while the field is
    // being edited must not put the saved URL back.
    await expect(page.locator('#mdai-fetch-url')).toHaveValue(api.url + '/data');
    await page.locator('#mdai-fetch-url').fill('');
    await expect(page.locator('#mdai-fetch-url')).toHaveValue('');
    await page.locator('#mdai-src-save').click();
    await expect.poll(() => fileText(file)).not.toContain('fetch:');
    await tab(page, 'edit');
    await page.locator('#mdai-instr-ta').fill(PROMPT + '\nKeep it short.\n');
    await expect(page.locator('#mdai-save-btn')).toHaveText('Save');
    await page.locator('#mdai-save-btn').click();
    await expect.poll(() => fileText(file)).toContain('Keep it short.');
    // Raw file mode edits the whole text; the server refuses a file that does not parse.
    await page.locator('#mdai-body .mdai-seg button[data-mode=raw]').click();
    await expect(page.locator('#mdai-body-ta')).toHaveValue(await fileText(file));
    await page.reload();
    await page.evaluate(() => (window as any).switchView('mdai'));
    await page.locator('.mdai-tr[data-path]').click();
    // refresh: open now runs on open; let that first run land.
    await expect.poll(() => api.prompts.length, { timeout: 15_000 }).toBe(1);
    await expect(page.locator('#mdai-runbanner.mdai-runbanner')).toHaveCount(0, { timeout: 20_000 });
    await tab(page, 'settings');
    await expect(page.locator('#mdai-set-title')).toHaveValue('Resume here');
    await expect(page.locator('#mdai-set-model')).toHaveValue('haiku');
    await page.locator('#mdai-set-refresh').uncheck();
    await page.locator('#mdai-set-save').click();
    await expect.poll(() => fileText(file)).toContain('refresh: manual');
    const afterOpen = api.prompts.length;
    expect(afterOpen, 'opening a refresh:open node ran it once').toBe(1);

    // ---- Run: double-click starts ONE run; the running state is the server's ---------
    api.mode.delayMs = 2500;
    await editInstruction(page);
    await page.locator('#mdai-run-btn').dblclick();
    await expect(page.locator('#mdai-runbanner.mdai-runbanner')).toBeVisible();
    await expect(page.locator('#mdai-run-btn')).toBeDisabled();
    // A second client asking for the same node joins the run in flight.
    await expect.poll(async () => (await (await request.get('/api/files/mdai/active?path=' + encodeURIComponent(file), { headers })).json()).running).toBe(true);
    const second = await request.post('/api/files/mdai/run', { headers, data: { path: file, wait: false } });
    expect(second.status()).toBe(202);
    expect((await second.json()).joined).toBe(true);
    await tab(page, 'runs');
    await expect(page.locator('#mdai-body .mdai-run-live')).toContainText('Running');
    await expect(page.locator('#mdai-runbanner.mdai-runbanner')).toHaveCount(0, { timeout: 20_000 });
    expect(api.prompts.length - afterOpen, 'double-click plus a second client = one model call').toBe(1);
    const row = page.locator('#mdai-body .mdai-run:not(.mdai-run-h)').first();
    await expect(row).toContainText('Completed');
    await expect(row).toContainText('haiku');
    await expect(row.locator('[data-l=In]')).toHaveText('1.2K');
    await expect(row.locator('[data-l=Out]')).toHaveText('567');
    await row.click();
    await expect(page.locator('#mdai-body .mdai-output')).toContainText('Where I Left Off');
    expect(api.prompts.at(-1)).toContain('"Where I Left Off" dashboard');
    expect(api.prompts.at(-1)).toContain('[source: amux:messages?days=30&limit=150&offset=10&kb=200]');
    await page.screenshot({ path: info.outputPath('mdai-preview.png') });

    // ---- Reload mid-run: the run is found again, then finishes ------------------
    api.mode.delayMs = 4000;
    await editInstruction(page);
    await page.locator('#mdai-run-btn').click();
    await expect(page.locator('#mdai-runbanner.mdai-runbanner')).toBeVisible();
    await page.reload();
    await page.evaluate(() => (window as any).switchView('mdai'));
    await expect(page.locator('.mdai-tr[data-path]')).toContainText('Running');
    await page.locator('.mdai-tr[data-path]').click();
    await expect(page.locator('#mdai-runbanner.mdai-runbanner')).toBeVisible();
    await expect(page.locator('#mdai-runbanner.mdai-runbanner')).toHaveCount(0, { timeout: 20_000 });
    await tab(page, 'runs');
    await expect(page.locator('#mdai-body .mdai-run:not(.mdai-run-h)').first()).toContainText('Completed');

    // ---- Stop: the call is abandoned and the run says Cancelled ------------------
    api.mode.delayMs = 30_000;
    await editInstruction(page);
    await page.locator('#mdai-run-btn').click();
    await expect(page.locator('#mdai-stop-btn')).toBeVisible();
    await page.locator('#mdai-stop-btn').click();
    await expect(page.locator('#mdai-runbanner.mdai-runbanner')).toHaveCount(0, { timeout: 10_000 });
    await tab(page, 'runs');
    await expect(page.locator('#mdai-body .mdai-run:not(.mdai-run-h)').first()).toContainText('Cancelled');

    // ---- A failing provider: the run says why -----------------------------------
    api.mode.delayMs = 0;
    api.mode.fail = 'Overloaded (stand-in)';
    await editInstruction(page);
    await page.locator('#mdai-run-btn').click();
    await expect(page.locator('#mdai-body .mdai-err')).toHaveCount(1, { timeout: 15_000 });
    await expect(page.locator('#mdai-body .mdai-err')).toContainText('Overloaded (stand-in)', { timeout: 15_000 });
    await tab(page, 'runs');
    await expect(page.locator('#mdai-body .mdai-run:not(.mdai-run-h)').first()).toContainText('Failed');
    api.mode.fail = undefined;
    if (narrow) {
      expect((await page.locator('#mdai-run-btn').boundingBox())!.height).toBeGreaterThanOrEqual(44);
      expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
    }
    await page.screenshot({ path: info.outputPath('mdai-runs.png') });

    // ---- Rename keeps the history; delete removes the node ------------------------
    await tab(page, 'settings');
    await page.locator('#mdai-set-name').fill('Resume');
    await page.locator('#mdai-set-save').click();
    await expect(page.locator('#mdai-title')).toHaveText('Resume.mdai');
    await tab(page, 'runs');
    await expect(page.locator('#mdai-overlay .mdai-tab[data-tab=runs] .mdai-tab-n')).toHaveText('5');
    await tab(page, 'settings');
    await page.getByRole('button', { name: 'Delete node' }).click();
    await expect(page.locator('#mdai-overlay')).not.toHaveClass(/active/);
    await expect(page.locator('#mdai-list')).toContainText('No MDAI nodes yet');
    await expect(access(join(dir, 'team', 'Resume.mdai'))).rejects.toThrow();
  } finally {
    await cleanup('restore MDAI prefs', () => Promise.all([pref('mdai_root', prior[0]), pref('mdai_api_base', prior[1])]));
    await cleanup('stop the stand-in Messages API', () => api.close());
    await cleanup('remove temp folder', () => rm(dir, { recursive: true, force: true }));
  }
});

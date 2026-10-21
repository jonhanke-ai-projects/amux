// Settings > Account > usage: a provider that is not connected offers its sign-in
// right there (MSG-91014, 2026-10-21). Before this the row said "Connect account"
// with no way to connect, and the Claude sign-in it now starts could never save
// its token (PATCH /api/settings/env refused CLAUDE_CODE_OAUTH_TOKEN with 400).
//
// Mocked: the Claude CLI's terminal (the e2e host has no Claude account) and the
// usage report (the real one needs a live subscription). Real: the button, the
// sign-in window, the token save to server.env, /api/identity seeing it, and the
// usage list refreshing when the sign-in completes.
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, Page, allowUnusedRoute } from './fixtures';

test.describe.configure({ mode: 'serial' });

const AUTH_URL = 'https://claude.com/cai/oauth/authorize?code=true&client_id=e2e&state=e2e';
const FAKE_TOKEN = 'sk-ant-oat01-' + 'e2eFakeSubscriptionToken_0123456789';

async function token(page: Page) {
  await page.waitForFunction(() => typeof (window as any)._AMUX_AUTH_TOKEN === 'string');
  const t = await page.evaluate(() => (window as any)._AMUX_AUTH_TOKEN as string);
  return t ? { Authorization: `Bearer ${t}` } : {};
}

function usage(connected: boolean) {
  return {
    providers: [
      connected
        ? { id: 'claude', label: 'Claude', available: true, metered: true,
            windows: [{ label: '5-hour session', used_percent: 12, remaining_percent: 88 }] }
        : { id: 'claude', label: 'Claude', available: false, cause: 'no_token',
            reason: 'No Claude subscription token on this host' },
      { id: 'codex', label: 'Codex', available: false, cause: 'no_token', reason: 'Not signed in' },
      { id: 'ollama', label: 'Ollama', available: true, metered: false },
      { id: 'muse', label: 'Muse Code', available: false, cause: 'account_quota_not_reported', reason: 'Not reported' },
    ],
  };
}

async function mockCli(page: Page) {
  const st = { codeTyped: false, created: 0, inputs: [] as string[] };
  await page.route('**/api/terminal/create', r => { st.created++; r.fulfill({ json: { id: 'e2e-login' } }); });
  await page.route('**/api/terminal/e2e-login/input', async r => {
    const data = Buffer.from(JSON.parse(r.request().postData() || '{}').data || '', 'base64').toString();
    st.inputs.push(data);
    if (data.startsWith('e2e-code')) st.codeTyped = true;
    r.fulfill({ json: { ok: true } });
  });
  await page.route('**/api/terminal/e2e-login/output**', async r => {
    await new Promise(res => setTimeout(res, 300));
    const out = st.codeTyped ? `\nYour OAuth token (valid for 1 year):\n\n${FAKE_TOKEN}\n`
      : `Browser didn't open? Use the url below to sign in:\n\n${AUTH_URL}\n\nPaste code here if prompted > `;
    r.fulfill({ json: { data: Buffer.from(out).toString('base64'), alive: true } });
  });
  await page.route('**/api/terminal/e2e-login', r => r.fulfill({ json: { ok: true } }));
  return st;
}

async function openUsage(page: Page) {
  await page.evaluate(() => { (window as any).toggleSettings(); (window as any)._settingsTab('account'); });
  await expect(page.locator('.usage-provider[data-provider="claude"]')).toBeVisible({ timeout: 15000 });
}

test('Claude: Sign in from the usage row runs the sign-in, saves the token and the row stops asking', async ({ page }) => {
  let connected = false;
  await page.route('**/api/usage', r => r.fulfill({ json: usage(connected) }));
  const cli = await mockCli(page);
  await page.goto('/');
  const h = await token(page);
  // A clean server: no subscription token saved.
  expect((await page.request.patch('/api/settings/env', { headers: h, data: { CLAUDE_CODE_OAUTH_TOKEN: '' } })).ok()).toBeTruthy();

  await openUsage(page);
  const row = page.locator('.usage-provider[data-provider="claude"]');
  await expect(row.locator('.usage-provider-status')).toHaveText('Connect account');
  await row.locator('summary').click();
  await expect(row).toContainText('No Claude subscription token on this host');
  const btn = row.locator('.usage-connect');
  await expect(btn).toHaveText('Sign in with Claude');
  // Only providers with a CLI sign-in get a button; one without says so.
  await page.locator('.usage-provider[data-provider="muse"] summary').click();
  await expect(page.locator('.usage-provider[data-provider="muse"] .usage-connect')).toHaveCount(0);
  await expect(page.locator('.usage-provider[data-provider="muse"] .usage-no-login')).toContainText('No dashboard sign-in for Muse Code yet');
  await expect(page.locator('.usage-provider[data-provider="ollama"] .usage-connect')).toHaveCount(0);

  await btn.click();
  await expect(page.locator('#apikey-setup-modal')).toBeVisible();
  await expect(page.locator('#model-login-status')).toHaveText('Open the page, sign in, then paste the code it shows you.', { timeout: 15000 });
  await expect(page.locator('#model-login-link')).toHaveAttribute('href', AUTH_URL);
  expect(cli.created).toBe(1);
  expect(cli.inputs[0]).toContain('claude setup-token');

  await page.locator('#model-login-paste').fill('e2e-code#e2e-state');
  connected = true;   // what the usage endpoint answers once the token is on the server
  await page.locator('#model-login-paste-wrap button').first().click();
  // Code and Enter are two writes (Claude Code reads one write as a paste).
  await expect.poll(() => cli.inputs.slice(1)).toEqual(['e2e-code#e2e-state', '\r']);

  // The token reaches server.env, the identity check sees it, the window closes,
  // and the usage row refreshes without a reload.
  await expect(page.locator('#apikey-setup-modal')).toBeHidden({ timeout: 30000 });
  await expect(page.getByText('Claude is connected. Your workers can use it now.')).toBeVisible();
  const env = await (await page.request.get('/api/settings/env', { headers: h })).json();
  expect(env.CLAUDE_CODE_OAUTH_TOKEN).toMatch(/^\*+6789$/);
  const id = await (await page.request.get('/api/identity', { headers: h })).json();
  expect(id.model_auth.claude.subscription_token).toBe(true);
  await expect(row.locator('.usage-provider-status')).toHaveText('88% left', { timeout: 15000 });
  // Connected, it still offers the same sign-in, as a switch (MSG-92525).
  await expect(row.locator('.usage-connect')).toHaveText('Switch Claude account');

  // Leave the e2e server as it was.
  expect((await page.request.patch('/api/settings/env', { headers: h, data: { CLAUDE_CODE_OAUTH_TOKEN: '' } })).ok()).toBeTruthy();
});

test('Codex: the usage row starts the ChatGPT device sign-in', async ({ page }) => {
  await page.route('**/api/usage', r => r.fulfill({ json: usage(false) }));
  const inputs: string[] = [];
  await page.route('**/api/terminal/create', r => r.fulfill({ json: { id: 'e2e-codex' } }));
  await page.route('**/api/terminal/e2e-codex/input', r => {
    inputs.push(Buffer.from(JSON.parse(r.request().postData() || '{}').data || '', 'base64').toString());
    r.fulfill({ json: { ok: true } });
  });
  await page.route('**/api/terminal/e2e-codex/output**', async r => {
    await new Promise(res => setTimeout(res, 300));
    // The recorded output of the codex version the cloud image pins.
    const out = fs.readFileSync(path.join(__dirname, 'fixtures/cli-login/codex-0.162.1.txt'));
    r.fulfill({ json: { data: out.toString('base64'), alive: true } });
  });
  await page.goto('/');
  await openUsage(page);
  const row = page.locator('.usage-provider[data-provider="codex"]');
  await row.locator('summary').click();
  await row.locator('.usage-connect').click();
  await expect(page.locator('#apikey-setup-modal')).toBeVisible();
  await expect(page.locator('#model-login-code')).toHaveText('ABCD-EFGH1', { timeout: 15000 });
  await expect(page.locator('#model-login-link')).toHaveAttribute('href', 'https://auth.openai.com/codex/device');
  expect(inputs[0]).toContain('codex login --device-auth');
});

// Ethan, 2026-10-21 (MSG-92525): "the same cli login flow for each". Gemini on an
// API key reads "Not reported" and had no button; a signed-in Codex whose usage
// reply was empty read "Connect account".
test('every provider with a CLI sign-in offers it, whatever its usage state', async ({ page }) => {
  await page.route('**/api/usage', r => r.fulfill({ json: { providers: [
    { id: 'claude', label: 'Claude', available: true, metered: true, windows: [{ label: '5-hour session', used_percent: 42, remaining_percent: 58 }] },
    { id: 'codex', label: 'Codex', available: false, cause: 'unexpected_shape', reason: 'Codex account usage returned no rate-limit snapshot.' },
    { id: 'gemini', label: 'Gemini', available: false, cause: 'account_quota_not_reported', reason: 'This Gemini authentication mode does not report an account-wide remaining subscription quota' },
    { id: 'ollama', label: 'Ollama', available: true, metered: false },
    { id: 'grok', label: 'Grok', available: true, metered: true, usage_unknown: true },
  ] } }));
  let created = 0;
  await page.route('**/api/terminal/create', r => { created++; r.fulfill({ json: { id: 'e2e-gemini' } }); });
  const typed: string[] = [];
  await page.route('**/api/terminal/e2e-gemini/input', r => { typed.push(Buffer.from(JSON.parse(r.request().postData() || '{}').data || '', 'base64').toString()); r.fulfill({ json: { ok: true } }); });
  await page.route('**/api/terminal/e2e-gemini/output**', async r => { await new Promise(res => setTimeout(res, 300)); r.fulfill({ json: { data: '', alive: true } }); });
  await page.route('**/api/terminal/e2e-gemini', r => r.fulfill({ json: { ok: true } }));
  allowUnusedRoute(page, '**/api/terminal/e2e-gemini');   // the window stays open
  await page.goto('/');
  await openUsage(page);
  const label = (id: string) => page.locator(`.usage-provider[data-provider="${id}"] .usage-connect`);
  await expect(label('claude')).toHaveText('Switch Claude account');
  await expect(page.locator('.usage-provider[data-provider="codex"] .usage-provider-status')).toHaveText('Usage not reported');
  await expect(label('codex')).toHaveText('Switch ChatGPT account');
  await expect(page.locator('.usage-provider[data-provider="gemini"] .usage-provider-status')).toHaveText('Not reported');
  await expect(label('gemini')).toHaveText('Sign in with Google (Gemini)');
  await expect(label('ollama')).toHaveCount(0);
  await expect(page.locator('.usage-provider[data-provider="ollama"] .usage-no-login')).toHaveCount(0);
  await expect(page.locator('.usage-provider[data-provider="grok"] .usage-no-login')).toContainText('No dashboard sign-in for Grok yet');
  await page.locator('.usage-provider[data-provider="gemini"] summary').click();
  await label('gemini').click();
  await expect(page.locator('#apikey-setup-modal')).toBeVisible();
  await expect.poll(() => created).toBe(1);
  await expect.poll(() => typed.join('')).toContain('gemini');
  await expect(page.locator('#model-login-status')).toHaveText(/Google \(Gemini\)/, { timeout: 15000 });
});

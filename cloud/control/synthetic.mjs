// The synthetic customer (docs/design/cloud-self-service.md section 12): every 6 hours and on
// every deploy, a real browser walks the paid path and reports each step to the control plane,
// which the weekly report reads. It deletes its own account at the end, so it leaves nothing.
//
//   CONTROL_URL=https://cloud.amux.io SYNTH_EMAIL=synthetic+<run>@amux.io CONTROL_ADMIN_TOKEN=... \
//   [SYNTH_COUPON=SYNTH100] node cloud/control/synthetic.mjs
//
// Checkout: Stripe test mode takes card 4242; live mode takes a 100% coupon (SYNTH_COUPON).
// In CONTROL_FAKE mode the fake checkout page is used and the run stops at the door hand-off,
// because there is no real server behind it.
import { chromium } from 'playwright';
import { createHmac } from 'node:crypto';

const CP = (process.env.CONTROL_URL || 'https://cloud.amux.io').replace(/\/$/, '');
const EMAIL = process.env.SYNTH_EMAIL || `synthetic+${Date.now()}@amux.io`;
const ADMIN = process.env.CONTROL_ADMIN_TOKEN;
const steps = [];
// Plan section 12 checks that need something only the owner holds. Listed every run, so the
// report shows the gap instead of a green suite that never tried them.
const NOT_RUN = {
  'T-PAY-1': 'STRIPE_TEST_SECRET_KEY', 'T-ACC-3': 'Apple key and test Google/GitHub accounts',
  'T-MAIL-1..7': 'outbound email and a test inbox', 'T-CLI-1': 'a Claude test subscription token',
  'T-CLI-2': 'a ChatGPT test account', 'T-CLI-3': 'a Google test account', 'T-TS-1..5': 'a test tailnet auth key',
  'T-KEY-1..3': 'test model API keys', 'T-TEAM-1..2': 'a test inbox for the invitee',
};
const t0 = Date.now();
let ok = false;

async function step(name, fn) {
  const s = Date.now();
  try {
    const detail = await fn();
    steps.push({ name, ok: true, ms: Date.now() - s, detail: detail ?? null });
  } catch (e) {
    steps.push({ name, ok: false, ms: Date.now() - s, error: String(e.message || e).slice(0, 300) });
    throw e;
  }
}

const health = await fetch(`${CP}/health`).then(r => r.json()).catch(() => ({}));
const fakeInfra = health.fake === true;              // no real server behind the checkout
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 390, height: 844 } });
if (ADMIN && health.payments !== 'live') {
  // Payments not live: the control plane only lets the admin start a server.
  await ctx.addCookies([{ name: 'amux_admin', value: ADMIN, url: CP }]);
}
const page = await ctx.newPage();
let fake = false, doorUrl = null, rawDoor = null, paidAt = 0, handoffAt = 0;
const capability = { passed: [], failed: [], not_run: NOT_RUN };
// Stripe test mode only: the dunning check changes a subscription's card and billing cycle.
const SK = process.env.STRIPE_SECRET_KEY || '';
async function stripe(method, path, params = {}) {
  const body = new URLSearchParams(params).toString();
  const r = await fetch(`https://api.stripe.com/v1${path}${method === 'GET' && body ? '?' + body : ''}`,
    { method, headers: { Authorization: `Bearer ${SK}`, 'Content-Type': 'application/x-www-form-urlencoded' },
      body: method === 'GET' ? undefined : body });
  const j = await r.json();
  if (j.error && !params._allowError) throw new Error(`stripe ${path}: ${j.error.message}`);
  return j;
}
async function billingStatus(want, ms = 90_000, also = () => true) {
  const t = Date.now();
  let last = null;
  while (Date.now() - t < ms) {
    last = (await (await page.request.get(`${CP}/api/me`)).json()).billing;
    if (last && last.status === want && also(last)) return last;
    await page.waitForTimeout(3000);
  }
  throw new Error(`billing never reached ${want}; last ${JSON.stringify(last)}`);
}
try {
  await step('open pricing link', async () => {
    await page.goto(`${CP}/?plan=solo&source=synthetic`);
    fake = page.url().includes('/fake/checkout/');
    return page.url().replace(/\?.*/, '');
  });
  await step('pay', async () => {
    if (fake) {
      await page.fill('input[name=email]', EMAIL);
      await Promise.all([page.waitForURL(/\/progress/, { timeout: 60_000 }), page.click('#pay')]);
      paidAt = Date.now();
      return 'fake checkout';
    }
    await page.fill('input[name=email]', EMAIL);
    if (process.env.SYNTH_COUPON) {
      await page.getByText('Add promotion code').click();
      await page.fill('input[name=promotionCode]', process.env.SYNTH_COUPON);
      await page.keyboard.press('Enter');
    } else {
      const card = page.frameLocator('iframe[name^="__privateStripeFrame"]').first();
      await page.fill('#cardNumber', '4242424242424242').catch(() => card.locator('[name=cardnumber]').fill('4242424242424242'));
      await page.fill('#cardExpiry', '12 / 34');
      await page.fill('#cardCvc', '123');
      await page.fill('#billingName', 'Synthetic Customer');
    }
    await Promise.all([page.waitForURL(/\/progress/, { timeout: 120_000 }), page.click('button[type=submit]')]);
    paidAt = Date.now();
    return process.env.SYNTH_COUPON ? 'coupon' : 'test card';
  });
  await step('server ready and door hand-off', async () => {
    doorUrl = await new Promise((resolve, reject) => {
      page.on('request', r => { if (r.url().includes('/_door/enter')) { rawDoor = r.url(); resolve(r.url().replace(/t=.*/, 't=<token>')); } });
      setTimeout(() => reject(new Error('no door hand-off within 15 minutes')), fakeInfra ? 20_000 : 15 * 60_000);
    });
    handoffAt = Date.now();
    return doorUrl;
  });
  if (!fakeInfra) {
    await step('dashboard loads', async () => {
      await page.waitForLoadState('domcontentloaded');
      // A visible element plus an API call through the door cookie. The old selector list only
      // matched a hidden modal, so it failed on a healthy server (first real door hand-off, 2026-10-14).
      await page.waitForSelector('#brand-header', { timeout: 60_000 });
      const api = await page.evaluate(async () => (await fetch('/api/sessions')).status);
      if (api !== 200) throw new Error(`/api/sessions through the door returned ${api}`);
      return new URL(page.url()).host;
    });
    await step('capability checks', async () => {
      const check = (id, ok, detail) => (ok ? capability.passed : capability.failed).push(detail ? `${id}: ${detail}` : id);
      const host = new URL(page.url()).host;
      check('T-PROV-1', paidAt > 0 && handoffAt - paidAt < 180_000, `paid to ready in ${Math.round((handoffAt - paidAt) / 1000)} s`);
      // T-SSO-1: the door cookie must not be readable by any other customer's host.
      const cookies = await ctx.cookies(`https://${host}/`);
      const loose = cookies.filter(c => c.domain.startsWith('.') || c.domain !== host).map(c => `${c.name}@${c.domain}`);
      check('T-SSO-1 cookie scope', cookies.length > 0 && loose.length === 0, loose.length ? `shared: ${loose.join(',')}` : `${cookies.length} host-only`);
      // T-SSO-1: a real token re-addressed to another server fails its signature.
      const t = new URL(rawDoor).searchParams.get('t');
      const [payload, mac] = t.split('.');
      const claims = JSON.parse(Buffer.from(payload, 'base64url').toString());
      const forged = Buffer.from(JSON.stringify({ ...claims, aud: 'someone-else' })).toString('base64url') + '.' + mac;
      const fresh = await browser.newContext();
      const r = await fresh.request.get(`https://${host}/_door/enter?t=${forged}`, { maxRedirects: 0 });
      check('T-SSO-1 forged token', r.status() === 401, `HTTP ${r.status()}`);
      // Locks: no cookie, no API; the dashboard sends a stranger to the door.
      const api = await fresh.request.get(`https://${host}/api/sessions`, { maxRedirects: 0 });
      check('door API lock', api.status() === 401, `HTTP ${api.status()}`);
      // A stranger's page load is sent to sign in: on a Cloud server that is the control plane
      // (?server=<slug>), elsewhere the door's own page. Sent as a browser navigation, since the
      // door answers API-shaped requests with a JSON 401 instead.
      const home = await fresh.request.get(`https://${host}/`, { maxRedirects: 0,
        headers: { Accept: 'text/html', 'Sec-Fetch-Mode': 'navigate' } });
      const to = home.headers().location || '';
      const locked = [302, 303].includes(home.status()) && (to.includes('/_door/') || to.startsWith(CP) && to.includes('server='));
      check('door page lock', locked, `HTTP ${home.status()} to ${to.replace(/\?.*/, '')}`);
      await fresh.close();
      if (!fake && SK.startsWith('sk_test_')) {
        check('T-PAY-1 test checkout', true, 'Stripe test card to a ready server');
        // T-DUN-1, the Stripe half: a real failed charge, then a fixed card. The 14 days of service and the
        // snapshot that follow run on our own clock and are covered with an injected clock in test_lifecycle.py.
        const cust = (await stripe('GET', '/customers', { email: EMAIL })).data[0];
        // T-PAY-1, replay half: Stripe redelivers events; the same checkout event a second time must not
        // start a second server.
        const events = (await stripe('GET', '/events', { type: 'checkout.session.completed', limit: '20' })).data;
        const ev = events.find(e => e.data.object.customer === cust.id);
        const raw = JSON.stringify(ev), ts = Math.floor(Date.now() / 1000);
        const sig = createHmac('sha256', process.env.STRIPE_WEBHOOK_SECRET || '').update(`${ts}.${raw}`).digest('hex');
        const replay = await page.request.post(`${CP}/webhooks/stripe`, { data: raw,
          headers: { 'Content-Type': 'application/json', 'Stripe-Signature': `t=${ts},v1=${sig}` } });
        const servers = (await (await page.request.get(`${CP}/api/me`)).json()).servers;
        check('T-PAY-1 webhook replay', replay.status() === 200 && servers.length === 1,
          `HTTP ${replay.status()}, ${servers.length} server(s) after replay`);
        const sub = (await stripe('GET', '/subscriptions', { customer: cust.id, status: 'active' })).data[0];
        const bad = await stripe('POST', '/payment_methods/pm_card_chargeCustomerFail/attach', { customer: cust.id });
        await stripe('POST', `/subscriptions/${sub.id}`, { default_payment_method: bad.id, billing_cycle_anchor: 'now',
          proration_behavior: 'none', payment_behavior: 'allow_incomplete' });
        // Stripe sends subscription.updated (past_due) a few seconds before invoice.payment_failed,
        // which is what records when the failure started; wait for both.
        const failing = await billingStatus('past_due', 90_000, b => !!b.failing_since);
        check('T-DUN-1 failed charge seen', !!failing.failing_since, 'past_due with failing_since');
        const good = await stripe('POST', '/payment_methods/pm_card_visa/attach', { customer: cust.id });
        await stripe('POST', `/subscriptions/${sub.id}`, { default_payment_method: good.id });
        const inv = (await stripe('GET', `/subscriptions/${sub.id}`)).latest_invoice;
        await stripe('POST', `/invoices/${inv}/pay`, { payment_method: good.id });
        const fixed = await billingStatus('active');
        check('T-DUN-1 fixed card restores', fixed.failing_since === null, 'active, failure cleared');
        delete capability.not_run['T-PAY-1'];
        capability.not_run['T-DUN-1 14 days then snapshot'] = 'injected clock in test_lifecycle.py, not a live run';
      }
      // T-PROV-1 is a speed target that depends on Hetzner's snapshot restore; it is reported in
      // capability.run every time but does not by itself mark the journey broken.
      const broken = capability.failed.filter(f => !f.startsWith('T-PROV-1'));
      if (broken.length) throw new Error(`failed: ${broken.join('; ')}`);
      return `${capability.passed.length} passed, ${Object.keys(capability.not_run).length} not run`;
    });
  }
  ok = true;
} catch (e) {
  await page.screenshot({ path: `synthetic-failure-${Date.now()}.png` }).catch(() => {});
}
// Always, once payment went through: a failed step must not leave a paid server running. Two
// runs leaked their servers this way and filled the Hetzner core quota (2026-10-15).
if (paidAt && !fakeInfra) {
  await step('delete the synthetic account', async () => {
    const r = await page.request.post(`${CP}/api/account/delete`, { data: { confirm: 'delete' } });
    if (r.status() !== 202) throw new Error(`delete returned ${r.status()}`);
    return 'deletion started';
  }).catch(() => { ok = false; });
}
await browser.close();
const run = { ok, fake: fakeInfra, fake_checkout: fake, ms: Date.now() - t0, steps, capability };
console.log(JSON.stringify(run, null, 1));
if (ADMIN) {
  const r = await fetch(`${CP}/api/admin/synthetic`, { method: 'POST', headers: { Authorization: `Bearer ${ADMIN}`, 'Content-Type': 'application/json' },
    body: JSON.stringify(run) }).catch(e => ({ status: 0, e }));
  if (r.status !== 200) console.error(`could not record the run (HTTP ${r.status}); the report will show it as not measured`);
}
process.exit(ok ? 0 : 1);

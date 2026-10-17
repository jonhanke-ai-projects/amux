// amux Cloud signup, as one Cloudflare Worker (owner design, 2026-10-17): a static page at
// amux.io/start, Stripe Checkout, and this function. No control-plane server and no database:
// a customer's server is standalone, and what links it to its subscription lives in Hetzner
// labels (amux-sub, amux-slug) and Stripe metadata.
//
//   POST /api/checkout       {plan, slug, email, pw_hash}  -> {url} of a Stripe Checkout session
//   GET  /api/slug?name=     -> {available, reason}
//   GET  /api/status?session_id=cs_...  -> {state: paying|creating|booting|ready|failed, url}
//   POST /api/billing        {email} -> emails a Stripe billing-portal link
//   POST /webhooks/stripe    checkout.session.completed: create the server and its DNS record
//                            customer.subscription.deleted: snapshot it, then (cron) delete it
//   cron every minute        ready emails; delete servers whose final snapshot is done;
//                            delete snapshots older than 30 days
//
// Bindings (cloud/start/deploy.sh): STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET, HCLOUD_TOKEN,
// CF_TOKEN, CF_ZONE_ID, GMAIL_OAUTH, GOLDEN_IMAGE, PLANS (JSON), MODE (live|test), TEST_KEY.

const SITE = 'https://amux.io';
const DOMAIN = 'amux.io';
const SNAPSHOT_DAYS = 30;
const RESERVED = new Set(('www api cloud control admin status mail app amux start docs blog help support billing accounts ' +
  'clerk clkmail demo sandbox login signin signup pricing dashboard console staging dev test cdn static assets ios ' +
  'android team security cp-test concierge').split(' '));
const BRANDING = { display_name: 'amux Cloud', icon: { type: 'url', url: 'https://amux.io/icon.png' },
  logo: { type: 'url', url: 'https://amux.io/icon.png' }, button_color: '#0b0f14' };

// ------------------------------------------------------------------ helpers
const json = (body, status = 200, extra = {}) => new Response(JSON.stringify(body), {
  status, headers: { 'content-type': 'application/json', 'cache-control': 'no-store', ...extra } });

function cors(req) {
  const o = req.headers.get('origin') || '';
  const ok = o === SITE || o === 'https://www.amux.io' || /^http:\/\/(localhost|127\.0\.0\.1)(:\d+)?$/.test(o);
  return ok ? { 'access-control-allow-origin': o, 'access-control-allow-headers': 'content-type,x-test-key',
                'access-control-allow-methods': 'GET,POST,OPTIONS', vary: 'origin' } : {};
}

function form(obj, prefix = '', out = []) {
  for (const [k, v] of Object.entries(obj)) {
    if (v === undefined || v === null) continue;
    const key = prefix ? `${prefix}[${k}]` : k;
    if (Array.isArray(v)) v.forEach((x, i) => typeof x === 'object' ? form(x, `${key}[${i}]`, out) : out.push([`${key}[${i}]`, String(x)]));
    else if (typeof v === 'object') form(v, key, out);
    else out.push([key, String(v)]);
  }
  return out;
}

async function stripe(env, method, path, params) {
  const q = params ? new URLSearchParams(form(params)).toString() : '';
  const url = 'https://api.stripe.com/v1' + path + (method === 'GET' && q ? '?' + q : '');
  const r = await fetch(url, { method, headers: { authorization: `Bearer ${env.STRIPE_SECRET_KEY}`,
    'content-type': 'application/x-www-form-urlencoded' }, body: method === 'GET' ? undefined : q });
  const d = await r.json();
  if (!r.ok) throw new Error(`stripe ${r.status}: ${d.error?.message || JSON.stringify(d)}`);
  return d;
}

async function hetzner(env, method, path, body) {
  const r = await fetch('https://api.hetzner.cloud/v1' + path, { method, headers: {
    authorization: `Bearer ${env.HCLOUD_TOKEN}`, 'content-type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined });
  const text = await r.text();
  const d = text ? JSON.parse(text) : {};
  if (!r.ok) { const e = new Error(`hetzner ${r.status}: ${d.error?.code || ''} ${d.error?.message || text}`); e.code = d.error?.code; throw e; }
  return d;
}

async function cf(env, method, path, body) {
  const r = await fetch('https://api.cloudflare.com/client/v4' + path, { method, headers: {
    authorization: `Bearer ${env.CF_TOKEN}`, 'content-type': 'application/json' }, body: body ? JSON.stringify(body) : undefined });
  const d = await r.json();
  if (!d.success) throw new Error(`cloudflare: ${JSON.stringify(d.errors)}`);
  return d.result;
}

const plans = env => JSON.parse(env.PLANS).plans;
const host = slug => `${slug}.${DOMAIN}`;
const log = (event, fields = {}) => console.log(JSON.stringify({ event, ...fields }));

function slugProblem(slug) {
  if (!/^[a-z][a-z0-9-]{2,29}$/.test(slug || '') || slug.endsWith('-')) return 'Use 3 to 30 lowercase letters, digits or dashes, starting with a letter.';
  if (RESERVED.has(slug) || slug.startsWith('synthetic')) return 'That name is reserved.';
  return null;
}

async function slugTaken(env, slug) {
  const recs = await cf(env, 'GET', `/zones/${env.CF_ZONE_ID}/dns_records?name=${host(slug)}`);
  if (recs.length) return true;
  const s = await hetzner(env, 'GET', `/servers?label_selector=amux-slug=${slug}`);
  return s.servers.length > 0;
}

// ------------------------------------------------------------------ Stripe signature
async function verifyStripe(env, body, header) {
  const parts = Object.fromEntries((header || '').split(',').map(p => p.split('=')));
  const t = parts.t, v1 = (header || '').split(',').filter(p => p.startsWith('v1=')).map(p => p.slice(3));
  if (!t || !v1.length || Math.abs(Date.now() / 1000 - Number(t)) > 300) return false;
  const key = await crypto.subtle.importKey('raw', new TextEncoder().encode(env.STRIPE_WEBHOOK_SECRET),
    { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  const sig = new Uint8Array(await crypto.subtle.sign('HMAC', key, new TextEncoder().encode(`${t}.${body}`)));
  const hex = [...sig].map(b => b.toString(16).padStart(2, '0')).join('');
  return v1.some(s => s.length === hex.length && [...s].reduce((a, c, i) => a | (c.charCodeAt(0) ^ hex.charCodeAt(i)), 0) === 0);
}

// ------------------------------------------------------------------ email (Gmail API as hello@amux.io)
async function sendMail(env, to, subject, text) {
  if (!env.GMAIL_OAUTH) { log('email.not_configured', { subject }); return false; }
  const c = JSON.parse(atob(env.GMAIL_OAUTH));
  const tok = await (await fetch(c.token_uri || 'https://oauth2.googleapis.com/token', { method: 'POST',
    body: new URLSearchParams({ client_id: c.client_id, client_secret: c.client_secret, refresh_token: c.refresh_token,
      grant_type: 'refresh_token' }) })).json();
  const msg = `From: amux Cloud <hello@amux.io>\r\nTo: ${to}\r\nSubject: ${subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n${text}\r\n\r\n-- \r\namux Cloud\r\n`;
  const raw = btoa(unescape(encodeURIComponent(msg))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  const r = await fetch('https://gmail.googleapis.com/gmail/v1/users/me/messages/send', { method: 'POST',
    headers: { authorization: `Bearer ${tok.access_token}`, 'content-type': 'application/json' }, body: JSON.stringify({ raw }) });
  log(r.ok ? 'email.sent' : 'email.failed', { subject, status: r.status });
  return r.ok;
}

// ------------------------------------------------------------------ provisioning
// First boot runs the bootstrap from main, fetched over the image's own checkout, so a server
// created from an older golden image still gets today's setup. The image's copy is the fallback.
const BOOT = [
  'set -a; . /etc/amux/standalone.env; set +a',
  'H=$(getent passwd amux | cut -d: -f6); B=$H/Dev/amux/cloud/desktop/bootstrap.sh',
  'runuser -u amux -- git -C $H/Dev/amux pull -q --ff-only >/var/log/amux-standalone.log 2>&1 || echo CHECKOUT_PULL_FAILED >>/var/log/amux-standalone.log',
  '[ -f $B ] || B=/opt/amux/bootstrap.sh',
  'bash $B >>/var/log/amux-standalone.log 2>&1 || echo BOOTSTRAP_FAILED >>/var/log/amux-standalone.log',
].join('; ');
function userData(slug, email, pwHash) {
  // Standalone: the golden image's bootstrap gets its domain and its one owner login. The
  // password arrives already hashed (door.py scrypt format); nothing else about the customer.
  const envFile = [`AMUX_DOMAIN='${host(slug)}'`, `AMUX_WEB_USERS='${email}:${pwHash}:${email}'`,
    `AMUX_SKIP_PACKAGES=1`].join('\n');
  return `#cloud-config
write_files:
  - path: /etc/amux/standalone.env
    permissions: '0600'
    content: |
${envFile.split('\n').map(l => '      ' + l).join('\n')}
runcmd:
  - [bash, -c, "${BOOT}"]
`;
}

async function goldenImage(env) {
  if (env.GOLDEN_IMAGE) return Number(env.GOLDEN_IMAGE);
  const d = await hetzner(env, 'GET', '/images?type=snapshot&label_selector=amux-golden&sort=created:desc');
  return d.images[0].id;
}

async function provision(env, session) {
  const meta = session.metadata || {};
  const sub = typeof session.subscription === 'string' ? session.subscription : session.subscription?.id;
  const email = (session.customer_details?.email || meta.email || '').toLowerCase();
  if (!sub || !meta.slug || !meta.pw_hash) throw new Error(`session ${session.id} lacks subscription or signup data`);
  const existing = await hetzner(env, 'GET', `/servers?label_selector=amux-sub=${sub}`);
  if (existing.servers.length) { log('provision.replay', { sub }); return existing.servers[0]; }   // Stripe redelivers
  let slug = meta.slug;
  if (await slugTaken(env, slug)) slug = `${slug}-${sub.slice(-4).toLowerCase()}`;   // lost a race since checkout
  const plan = plans(env).find(p => p.id === meta.plan) || plans(env)[0];
  const image = await goldenImage(env);
  const labels = { 'amux-cloud': '1', 'amux-standalone': '1', 'amux-sub': sub, 'amux-slug': slug,
                   'amux-plan': plan.id, 'amux-mode': env.MODE };
  let srv, lastErr;
  for (const location of ['fsn1', 'nbg1', 'hel1']) {
    try {
      srv = (await hetzner(env, 'POST', '/servers', { name: `amux-${slug}`, server_type: plan.server_type, image,
        location, labels, user_data: userData(slug, email, meta.pw_hash), start_after_create: true })).server;
      break;
    } catch (e) { lastErr = e; if (!['resource_unavailable', 'placement_error'].includes(e.code)) throw e; }
  }
  if (!srv) throw lastErr;
  // Hetzner's daily backups (7 kept, +20% of the server price): a restorable copy of the whole disk
  // without anything on the server to maintain. A failure here is logged and retried by the cron.
  await hetzner(env, 'POST', `/servers/${srv.id}/actions/enable_backup`).catch(e => log('backup.enable_failed', { slug, error: String(e).slice(0, 200) }));
  const ip = srv.public_net.ipv4.ip;
  await cf(env, 'POST', `/zones/${env.CF_ZONE_ID}/dns_records`, { type: 'A', name: host(slug), content: ip,
    proxied: false, ttl: 60, comment: `amux cloud ${sub}` });
  // The hash has done its job (it is on the server now); an empty value removes it from Stripe.
  await stripe(env, 'POST', `/subscriptions/${sub}`, { metadata: { slug, hetzner_id: srv.id, pw_hash: '' } });
  log('provision.created', { sub, slug, server: srv.id, plan: plan.id, mode: env.MODE });
  return srv;
}

async function deprovision(env, sub) {
  // Ended subscription: power off, keep a snapshot for 30 days (resubscribing can restore it),
  // and let the cron delete the server and its address once the snapshot exists.
  const d = await hetzner(env, 'GET', `/servers?label_selector=amux-sub=${sub}`);
  for (const s of d.servers) {
    if (s.labels['amux-ending']) continue;
    const expires = Math.floor(Date.now() / 1000) + SNAPSHOT_DAYS * 86400;
    const img = await hetzner(env, 'POST', `/servers/${s.id}/actions/create_image`, { type: 'snapshot',
      description: `amux cloud final ${s.labels['amux-slug']}`, labels: { 'amux-cloud-final': '1',
        'amux-sub': sub, 'amux-slug': s.labels['amux-slug'], 'amux-expires': String(expires) } });
    await hetzner(env, 'PUT', `/servers/${s.id}`, { labels: { ...s.labels, 'amux-ending': String(img.image.id) } });
    log('deprovision.snapshot', { sub, server: s.id, image: img.image.id });
  }
}

// ------------------------------------------------------------------ status
async function status(env, sessionId) {
  const s = await stripe(env, 'GET', `/checkout/sessions/${sessionId}`);
  if (s.status !== 'complete') return { state: 'paying' };
  const sub = s.subscription;
  const d = await hetzner(env, 'GET', `/servers?label_selector=amux-sub=${sub}`);
  if (!d.servers.length) return { state: 'creating' };
  const srv = d.servers[0], slug = srv.labels['amux-slug'], url = `https://${host(slug)}`;
  if (srv.labels['amux-ready'] === '1') return { state: 'ready', url, email: s.customer_details?.email };
  return { state: (await isUp(slug)) ? 'ready' : 'booting', url, email: s.customer_details?.email };
}

async function isUp(slug) {
  try {
    const r = await fetch(`https://${host(slug)}/_door/login`, { redirect: 'manual', cf: { cacheTtl: 0 } });
    return r.status === 200;
  } catch { return false; }
}

// ------------------------------------------------------------------ cron
async function tick(env) {
  const d = await hetzner(env, 'GET', `/servers?label_selector=amux-standalone=1,amux-mode=${env.MODE}`);
  for (const s of d.servers) {
    const slug = s.labels['amux-slug'];
    if (s.labels['amux-ending']) {
      const img = await hetzner(env, 'GET', `/images/${s.labels['amux-ending']}`).catch(() => null);
      if (img && img.image.status === 'available') {
        await hetzner(env, 'DELETE', `/servers/${s.id}`);
        for (const r of await cf(env, 'GET', `/zones/${env.CF_ZONE_ID}/dns_records?name=${host(slug)}`))
          await cf(env, 'DELETE', `/zones/${env.CF_ZONE_ID}/dns_records/${r.id}`);
        log('deprovision.deleted', { slug, server: s.id });
      }
      continue;
    }
    if (s.labels['amux-ready'] !== '1' && await isUp(slug)) {
      const sub = await stripe(env, 'GET', `/subscriptions/${s.labels['amux-sub']}`, { expand: ['customer'] });
      const email = sub.customer?.email;
      if (email && !email.startsWith('synthetic+')) {
        await sendMail(env, email, 'Your amux server is ready',
          `Your server is ready: https://${host(slug)}\r\n\r\nSign in with this email address and the password you chose at signup. ` +
          `Then connect a model: sign in with Claude, ChatGPT or Google, or paste an API key.\r\n\r\n` +
          `Billing and cancellation: ${SITE}/start/billing/`);
      }
      await hetzner(env, 'PUT', `/servers/${s.id}`, { labels: { ...s.labels, 'amux-ready': '1' } });
      log('provision.ready', { slug, ms_since_create: Date.now() - Date.parse(s.created) });
    }
  }
  await monitor(env, d.servers);
  const imgs = await hetzner(env, 'GET', '/images?type=snapshot&label_selector=amux-cloud-final=1');
  for (const i of imgs.images) {
    if (Number(i.labels['amux-expires'] || 0) * 1000 < Date.now()) {
      await hetzner(env, 'DELETE', `/images/${i.id}`);
      log('snapshot.expired', { image: i.id, slug: i.labels['amux-slug'] });
    }
  }
}

// ------------------------------------------------------------------ monitor
// Each ready server is checked every 10 minutes (staggered by id) through its public, secret-free
// /_door/status: is it reachable, did its last self-update succeed, is the updater still running,
// are backups on. Two bad checks in a row email the operator; recovery emails again. State lives
// in the server's labels (amux-fail = consecutive bad checks, amux-alert = when we last emailed).
const OPERATOR = 'hello@amux.io';
async function serverProblem(env, s) {
  if (!s.backup_window) return 'backups are off';
  let st;
  try {
    const r = await fetch(`https://${host(s.labels['amux-slug'])}/_door/status`, { cf: { cacheTtl: 0 } });
    if (r.status !== 200) return `status page answered HTTP ${r.status}`;
    st = await r.json();
  } catch (e) { return `unreachable (${String(e).slice(0, 80)})`; }
  const age = Date.now() / 1000 - (st.checked_at || 0);
  if (!st.live_commit) return 'amux-server is not answering behind the door';
  if (st.verdict === 'unknown') return Date.now() - Date.parse(s.created) > 30 * 60e3 ? 'self-updater has never reported' : '';
  if (age > 3 * 3600) return `self-updater last reported ${Math.round(age / 3600)}h ago`;
  if (String(st.verdict).startsWith('failed')) return `self-update ${st.verdict}: ${st.detail || ''}`;
  return '';
}
async function monitor(env, servers) {
  const minute = new Date().getUTCMinutes();
  for (const s of servers) {
    if (s.labels['amux-ending'] || s.labels['amux-ready'] !== '1' || (s.id + minute) % 10) continue;
    if (!s.backup_window) await hetzner(env, 'POST', `/servers/${s.id}/actions/enable_backup`).catch(() => {});
    const problem = await serverProblem(env, s).catch(e => `monitor error ${String(e).slice(0, 80)}`);
    const slug = s.labels['amux-slug'], fails = Number(s.labels['amux-fail'] || 0);
    const alerted = Number(s.labels['amux-alert'] || 0), now = Math.floor(Date.now() / 1000);
    const labels = { ...s.labels };
    if (problem) {
      labels['amux-fail'] = String(fails + 1);
      log('monitor.problem', { slug, problem, consecutive: fails + 1, mode: env.MODE });
      if (fails + 1 >= 2 && now - alerted > 6 * 3600) {
        await sendMail(env, OPERATOR, `[amux cloud ${env.MODE}] ${slug}: ${problem}`.slice(0, 150),
          `Server ${host(slug)} (Hetzner ${s.id}, plan ${s.labels['amux-plan']}) has failed ${fails + 1} checks in a row.\r\n\r\n` +
          `Problem: ${problem}\r\n\r\nStatus page: https://${host(slug)}/_door/status`);
        labels['amux-alert'] = String(now);
      }
    } else {
      if (alerted) await sendMail(env, OPERATOR, `[amux cloud ${env.MODE}] ${slug} recovered`, `${host(slug)} passes its checks again.`);
      delete labels['amux-fail']; delete labels['amux-alert'];
      log('monitor.ok', { slug });
    }
    if (JSON.stringify(labels) !== JSON.stringify(s.labels)) await hetzner(env, 'PUT', `/servers/${s.id}`, { labels });
  }
}

// ------------------------------------------------------------------ routes
async function route(req, env) {
  const url = new URL(req.url);
  const p = url.pathname;
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors(req) });
  if (p === '/health') return json({ ok: true, mode: env.MODE, measured: true });

  if (p === '/api/slug' && req.method === 'GET') {
    const name = (url.searchParams.get('name') || '').toLowerCase();
    const problem = slugProblem(name);
    if (problem) return json({ available: false, reason: problem }, 200, cors(req));
    const taken = await slugTaken(env, name);
    return json({ available: !taken, reason: taken ? 'That name is taken.' : '' }, 200, cors(req));
  }

  if (p === '/api/checkout' && req.method === 'POST') {
    if (env.MODE === 'test' && req.headers.get('x-test-key') !== env.TEST_KEY)
      return json({ error: 'test mode needs its key' }, 403, cors(req));   // test cards must not buy real servers
    const b = await req.json().catch(() => ({}));
    const slug = (b.slug || '').toLowerCase(), email = (b.email || '').trim().toLowerCase();
    const plan = plans(env).find(x => x.id === b.plan);
    const problem = slugProblem(slug) || (!plan && 'Pick a plan.') ||
      (!/^[a-z0-9._+-]+@[a-z0-9-]+(\.[a-z0-9-]+)+$/.test(email) && 'Enter a valid email address.') ||
      (!/^scrypt\$[A-Za-z0-9+/=]{20,40}\$[A-Za-z0-9+/=]{40,50}$/.test(b.pw_hash || '') && 'Password could not be prepared; reload and try again.');
    if (problem) return json({ error: problem }, 400, cors(req));
    if (await slugTaken(env, slug)) return json({ error: 'That name is taken.' }, 409, cors(req));
    const price = (await stripe(env, 'GET', '/prices', { lookup_keys: [plan.stripe_lookup_key], active: 'true' })).data[0];
    const meta = { slug, plan: plan.id, email, pw_hash: b.pw_hash, source: (b.source || 'start').slice(0, 40) };
    const api = env.MODE === 'test' ? `&api=${encodeURIComponent(url.origin)}` : '';
    const s = await stripe(env, 'POST', '/checkout/sessions', {
      mode: 'subscription', line_items: [{ price: price.id, quantity: 1 }], customer_email: email,
      allow_promotion_codes: true, automatic_tax: { enabled: true }, billing_address_collection: 'auto',
      branding_settings: BRANDING, metadata: meta, subscription_data: { metadata: meta },
      success_url: `${SITE}/start/welcome/?session_id={CHECKOUT_SESSION_ID}${api}`,
      cancel_url: `${SITE}/start/?canceled=1`,
    });
    log('checkout.created', { slug, plan: plan.id, mode: env.MODE });
    return json({ url: s.url }, 200, cors(req));
  }

  if (p === '/api/status' && req.method === 'GET') {
    const sid = url.searchParams.get('session_id') || '';
    if (!/^cs_(test|live)_[A-Za-z0-9]+$/.test(sid)) return json({ error: 'bad session' }, 400, cors(req));
    return json(await status(env, sid), 200, cors(req));
  }

  if (p === '/api/billing' && req.method === 'POST') {
    // The answer is the same whether or not the address has an account, so it reveals nothing.
    const email = ((await req.json().catch(() => ({}))).email || '').trim().toLowerCase();
    const custs = email ? (await stripe(env, 'GET', '/customers', { email, limit: 10 })).data : [];
    for (const c of custs) {
      const subs = (await stripe(env, 'GET', '/subscriptions', { customer: c.id, status: 'all', limit: 10 })).data;
      if (!subs.some(s => s.metadata?.slug)) continue;
      const portal = await stripe(env, 'POST', '/billing_portal/sessions', { customer: c.id, return_url: `${SITE}/start/billing/`,
                                                                            ...(env.PORTAL_CONFIG ? { configuration: env.PORTAL_CONFIG } : {}) });
      await sendMail(env, email, 'Manage your amux Cloud billing',
        `Open this link within an hour to change your card, see invoices or cancel:\r\n\r\n${portal.url}`);
      break;
    }
    return json({ ok: true }, 200, cors(req));
  }

  if (p === '/webhooks/stripe' && req.method === 'POST') {
    const body = await req.text();
    if (!(await verifyStripe(env, body, req.headers.get('stripe-signature')))) return json({ error: 'bad signature' }, 400);
    const ev = JSON.parse(body);
    const o = ev.data.object;
    if (ev.type === 'checkout.session.completed' && o.mode === 'subscription' && o.metadata?.pw_hash &&
        ['paid', 'no_payment_required'].includes(o.payment_status)) {
      await provision(env, o);
    } else if (ev.type === 'customer.subscription.deleted' && o.metadata?.slug) {
      await deprovision(env, o.id);
    }
    return json({ received: true });
  }
  return json({ error: 'not found' }, 404, cors(req));
}

export default {
  async fetch(req, env) {
    try { return await route(req, env); }
    catch (e) { log('error', { path: new URL(req.url).pathname, error: String(e.message || e).slice(0, 300) });
                return json({ error: 'Something went wrong on our side. Please try again.' }, 500, cors(req)); }
  },
  async scheduled(_ev, env, ctx) {
    ctx.waitUntil(tick(env).catch(e => log('cron.error', { error: String(e.message || e).slice(0, 300) })));
  },
};

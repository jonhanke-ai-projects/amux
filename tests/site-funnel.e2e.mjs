import { chromium } from 'playwright';
// Run against a local server of site/:  (cd site && python3 -m http.server 8731) &
// then: node tests/site-funnel.e2e.mjs   (SITE=http://host:port to point elsewhere)
// PostHog is kept as its stub, so no event reaches production.
const B = process.env.SITE || 'http://localhost:8731';
const b = await chromium.launch();
let pass = 0, fail = 0;
const ok = (c, m) => { c ? pass++ : fail++; console.log((c ? 'ok   ' : 'FAIL ') + m); };
async function page(opts = {}) {
  const ctx = await b.newContext({ viewport: opts.mobile ? { width: 390, height: 844 } : { width: 1440, height: 900 } });
  await ctx.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: B });
  const p = await ctx.newPage();
  const errs = []; p.on('pageerror', e => errs.push(e.message));
  // Keep PostHog a stub so nothing reaches production and calls stay inspectable.
  await p.route(/posthog\.com|cloud\.amux\.io/, r => r.abort());
  p.errs = errs;
  return p;
}
const calls = p => p.evaluate(() => Array.from(window.posthog).filter(Array.isArray).map(c => [c[0], c[1], c[2]]));
const evs = async (p, name) => (await calls(p)).filter(c => c[0] === 'capture' && c[1] === name).map(c => c[2]);

// Homepage, arriving from ChatGPT
let p = await page();
await p.goto(B + '/', { referer: 'https://chatgpt.com/' });
await p.evaluate(() => document.addEventListener('click', e => e.preventDefault()));
let c = await calls(p);
const once = c.find(x => x[0] === 'register_once');
ok(once && once[1].first_source === 'ai' && once[1].first_landing === '/', 'first touch from chatgpt.com is source ai, landing /');
ok(c.some(x => x[0] === 'register_for_session' && x[1].session_source === 'ai'), 'session source registered');
await p.click('.site-header a[href="https://github.com/mixpeek/amux"]');
await p.click('.hero a[href="https://github.com/mixpeek/amux"]');
await p.click('.hero a[href^="/start/"]');
await p.click('#hero-watch');
await p.click('.site-header a[href="#get"]');
await p.click('a[href="/start/?plan=team"]');
await p.click('.hero .copy-btn').catch(() => {});
let cta = await evs(p, 'cta_click');
const has = (g, s) => cta.some(e => e.goal === g && e.spot === s);
ok(has('github', 'header'), 'header star click -> goal github, spot header');
ok(has('github', 'hero'), 'hero star click -> goal github, spot hero');
ok(has('cloud', 'hero'), 'hero cloud click -> goal cloud, spot hero');
ok(has('walkthrough', 'hero'), 'hero walkthrough -> goal walkthrough');
ok(has('pricing', 'header'), 'Get started -> goal pricing, spot header');
ok(cta.some(e => e.goal === 'cloud' && e.spot === 'get' && e.plan === 'team'), 'plan link -> goal cloud, spot get, plan team');
const ic = await evs(p, 'install_copy');
ok(ic.length === 1 && ic[0].spot === 'hero', 'hero copy -> install_copy spot hero (' + JSON.stringify(ic) + ')');
await p.click('a[href="/docs/"] >> nth=0').catch(() => {});
ok((await evs(p, 'cta_click')).length === cta.length, 'non-goal link (docs) sends no cta_click');
ok(p.errs.length === 0, 'no JS errors on homepage ' + p.errs.join('|'));

// A content page from Google, mobile
p = await page({ mobile: true });
await p.goto(B + '/guides/harness-engineering/', { referer: 'https://www.google.com/' });
await p.evaluate(() => document.addEventListener('click', e => e.preventDefault()));
c = await calls(p);
ok(c.find(x => x[0] === 'register_once')?.[1].first_source === 'search', 'google referrer -> search');
await p.click('.site-header a[href*="github.com/mixpeek/amux"]');
cta = await evs(p, 'cta_click');
ok(cta.length === 1 && cta[0].goal === 'github' && cta[0].spot === 'header', 'mobile guide header star -> github/header');
ok(p.errs.length === 0, 'no JS errors on guide');

// Internal navigation does not overwrite first touch
p = await page();
await p.goto(B + '/pricing/', { referer: B + '/' });
c = await calls(p);
ok(!c.some(x => x[0] === 'register_once'), 'internal referrer registers no first touch');

// Signup page
p = await page();
await p.goto(B + '/start/?plan=solo');
await p.focus('#slug');
ok((await evs(p, 'signup_form_start')).length === 1, 'focusing the form -> signup_form_start');
await p.focus('#email');
ok((await evs(p, 'signup_form_start')).length === 1, 'signup_form_start fires once');
await p.fill('#slug', 'funneltest'); await p.fill('#email', 'a@example.com'); await p.fill('#pw', 'abcdefghijk');
await p.click('#go');
await p.waitForFunction(() => !document.getElementById('go').disabled, null, { timeout: 20000 });
const sub = await evs(p, 'signup_submit');
ok(sub.length === 1 && sub[0].plan === 'solo' && sub[0].test === false, 'submit -> signup_submit plan solo ' + JSON.stringify(sub));
const er = await evs(p, 'signup_error');
ok(er.length === 1, 'blocked checkout API -> signup_error ' + JSON.stringify(er));
ok(p.errs.length === 0, 'no JS errors on /start/ ' + p.errs.join('|'));

// Welcome page: paid once per order
p = await page();
await p.goto(B + '/start/welcome/?session_id=cs_test_funnel');
ok((await evs(p, 'signup_paid')).length === 1, 'welcome -> signup_paid');
await p.reload();
ok((await evs(p, 'signup_paid')).length === 0, 'reload does not re-send signup_paid');
p = await page();
await p.goto(B + '/start/welcome/?session_id=cs_test_2&tk=x');
const tp = await evs(p, 'signup_paid');
ok(tp.length === 1 && tp[0].test === true, 'test checkout is flagged test:true');
await b.close();
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);

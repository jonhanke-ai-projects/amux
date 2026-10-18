// The held-email card shows the WHOLE draft (Ethan, 2026-10-12: "did this get
// cut off"). preview.body is capped at 2000 characters, so a 4510-character
// reply showed its first half and Edit copied only that half into the box,
// where Approve would have sent it. The card now renders review.body, the
// frozen message, and withholds Edit when only the preview is available.
import { test, expect } from './fixtures';

const HEAD = 'Hi Anthony,\n\nResults from your eight photos are below.\n\n';
const RESULTS = Array.from({ length: 120 }, (_, i) => `   ${(i % 5) + 1}) 0.8${i % 10}  79081${i}  Ring ${i}`).join('\n');
const TAIL = '\n\nOn pricing, two options:\n\n- Build plan: $25 a month.\n\nDo your customers send mostly product shots?';
const FULL = HEAD + RESULTS + TAIL;

function approval(review: object | undefined) {
  return {
    id: 'apr_fullbody0001', session: 'gtm-engine', age_s: 60, expires: false, expires_in_s: null, requests: 1,
    preview: { endpoint: 'reply', from: 'info@example.com', to: 'buyer@example.org', subject: 'Re: Pricing',
      reply_all: false, body: FULL.slice(0, 2000), external_recipients: ['buyer@example.org'] },
    ...(review ? { review } : {}),
  };
}

async function open(page: any, pending: object[]) {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.route(/\/api\/email\/approvals(?:\?.*)?$/, (r: any) => r.fulfill({ json: { pending } }));
  await page.route(/\/api\/grants(?:\?.*)?$/, (r: any) => r.fulfill({ json: { grants: [] } }));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any)._approvalsRefresh === 'function');
  await page.evaluate(() => (window as any)._approvalsRefresh());
}

test('the held-email card shows the full draft and Edit starts from it', async ({ page }) => {
  expect(FULL.length).toBeGreaterThan(2000);
  await open(page, [approval({ body: FULL, body_complete: true, attachment_count: 0, complete: true })]);
  const body = page.locator('#appr-body-apr_fullbody0001');
  await expect(body).toContainText('Do your customers send mostly product shots?');
  await expect(body).toContainText('$25 a month');
  await expect(page.locator('#email-approvals-banner')).not.toContainText('Showing only the first');
  await page.locator('#email-approvals-banner button', { hasText: 'Edit' }).click();
  const ta = page.locator('#appr-edit-apr_fullbody0001');
  await expect(ta).toBeVisible();
  expect((await ta.inputValue()).trim()).toBe(FULL.trim());
});

test('with only the capped preview, the card says so and Edit is withheld', async ({ page }) => {
  await open(page, [approval(undefined)]);
  const banner = page.locator('#email-approvals-banner');
  await expect(banner).toContainText('Showing only the first 2000 characters');
  await expect(banner.locator('button', { hasText: 'Edit' })).toBeDisabled();
});

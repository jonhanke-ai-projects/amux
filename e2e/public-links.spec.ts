// The Pages tab's three kinds (static_files/public_links.rs): an app inside
// amux for the owner, the app shared publicly, and a public folder. The real
// server and a real loopback app; everything is made and removed in the UI,
// and the public links are opened with no amux credentials.
import { test, expect } from './fixtures';
import * as fs from 'fs';
import * as http from 'http';
import * as os from 'os';
import * as path from 'path';
import type { AddressInfo } from 'net';

test('Pages provisions an app inside amux, a public app and a public folder', async ({ page, playwright }, testInfo) => {
  const app = http.createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'text/html' });
    res.end(`<h1>shared app</h1><p id="cookie">${(req.headers.cookie || '').replace(/</g, '')}</p>`);
  });
  await new Promise<void>(r => app.listen(0, '127.0.0.1', () => r()));
  const port = (app.address() as AddressInfo).port;
  // A visible folder under HOME: amux never publishes hidden paths.
  const dir = fs.mkdtempSync(path.join(os.homedir(), 'pagesfiles-'));
  fs.writeFileSync(path.join(dir, 'report.txt'), 'quarterly report');
  fs.writeFileSync(path.join(dir, '.secret'), 'never public');
  const tag = testInfo.project.name;
  const modalBtn = (name: string) => page.locator('#modal-btns').getByRole('button', { name });
  const base = () => new URL(page.url()).origin;
  const anon = await playwright.request.newContext({ ignoreHTTPSErrors: true });
  try {
    // A frame cannot send the bearer token: sign this browser in with the
    // owner cookie, as port-proxy.spec.ts does.
    await page.goto('/');
    await page.waitForFunction(() => !!(window as any)._AMUX_AUTH_TOKEN, { timeout: 20000 });
    const token = await page.evaluate(() => (window as any)._AMUX_AUTH_TOKEN as string);
    await page.goto('/?_token=' + encodeURIComponent(token));
    await page.waitForURL(/_fresh=1/, { timeout: 20000 });
    await page.goto('/#view=pages');
    for (const h of ['Inside amux', 'Public apps', 'Public files']) await expect(page.getByRole('heading', { name: h })).toBeVisible();

    // 1. Inside amux: listed, and opens in the amux UI.
    await page.getByRole('button', { name: 'Add app' }).click();
    await page.locator('#pub-new-target').fill(String(port));
    await page.locator('#pub-new-title').fill(`inside ${tag}`);
    await modalBtn('Add').click();
    const inside = page.locator('#pub-internal .pub-row', { hasText: `inside ${tag}` });
    await expect(inside).toContainText(`/proxy/${port}/`);
    await inside.click();
    await expect(page.locator('#pages-open')).toBeVisible();
    await expect(page.frameLocator('.pub-app-frame').locator('h1')).toHaveText('shared app');
    await expect(page.locator('#pages-page-actions')).toBeHidden();
    await page.getByRole('button', { name: 'Back to all pages' }).click();

    // 2. A public app: anyone, no amux credential reaches it.
    await page.getByRole('button', { name: 'Share an app' }).click();
    await page.locator('#pub-new-target').fill(String(port));
    await page.locator('#pub-new-title').fill(`public ${tag}`);
    await modalBtn('Create link').click();
    const pub = page.locator('#pub-apps .pub-row', { hasText: `public ${tag}` });
    const appHref = await pub.locator('.pub-url').getAttribute('href');
    expect(appHref).toMatch(/\/pub\/[0-9a-f]{32}\/$/);
    const appUrl = new URL(appHref!, base()).toString();
    const opened = await anon.get(appUrl, { headers: { Cookie: 'amux_member=m; appsid=1' } });
    expect(opened.status()).toBe(200);
    const body = await opened.text();
    expect(body).toContain('<h1>shared app</h1>');
    expect(body).not.toContain('amux_member');
    expect(opened.headers()['content-security-policy']).toContain('sandbox');

    // 3. A public folder: its files, never its hidden ones.
    await page.getByRole('button', { name: 'Share a file' }).click();
    await page.locator('#pub-new-target').fill(dir);
    await page.locator('#pub-new-title').fill(`files ${tag}`);
    await modalBtn('Create link').click();
    const files = page.locator('#pub-files .pub-row', { hasText: `files ${tag}` });
    const folderUrl = new URL((await files.locator('.pub-url').getAttribute('href'))!, base()).toString();
    const listing = await (await anon.get(folderUrl)).text();
    expect(listing).toContain('report.txt');
    expect(listing).not.toContain('.secret');
    expect(await (await anon.get(folderUrl + 'report.txt')).text()).toBe('quarterly report');
    expect((await anon.get(folderUrl + '.secret')).status()).toBe(404);
    // Traversal is pinned in tests/public_links.rs with raw paths; a client
    // here normalizes `../` away before sending it.

    // One row of controls per entry on a phone.
    for (const row of [pub, files]) {
      const box = (await row.boundingBox())!;
      expect(box.width).toBeLessThanOrEqual(page.viewportSize()!.width);
      const off = (await row.getByRole('button', { name: 'Turn off' }).boundingBox())!;
      expect(off.y).toBeLessThan(box.y + box.height);
    }

    // Turned off in the UI, gone for everyone.
    for (const [row, url] of [[pub, appUrl], [files, folderUrl]] as const) {
      await row.getByRole('button', { name: 'Turn off' }).click();
      await modalBtn('Turn off').click();
      await expect(row).toHaveCount(0);
      expect((await anon.get(url)).status()).toBe(404);
    }
    await inside.getByRole('button', { name: 'Remove' }).click();
    await modalBtn('Remove').click();
    await expect(inside).toHaveCount(0);
  } finally {
    await anon.dispose();
    app.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

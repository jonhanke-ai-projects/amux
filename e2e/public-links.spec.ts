// Public links (static_files/public_links.rs): an app on a port, for anyone
// with the link, listed under Pages beside the pages themselves. The real
// server and a real loopback app: the link is created in the UI, opened with
// no amux credentials, and turned off in the UI.
import { test, expect } from './fixtures';
import * as http from 'http';
import type { AddressInfo } from 'net';

test('a public link is made, opened without amux sign-in, and turned off from Pages', async ({ page, playwright }, testInfo) => {
  const app = http.createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'text/html' });
    res.end(`<h1>shared app</h1><p id="cookie">${(req.headers.cookie || '').replace(/</g, '')}</p>`);
  });
  await new Promise<void>(r => app.listen(0, '127.0.0.1', () => r()));
  const port = (app.address() as AddressInfo).port;
  const title = `demo ${testInfo.project.name}`;
  try {
    await page.goto('/#view=pages');
    await expect(page.getByRole('heading', { name: 'Public links' })).toBeVisible();
    await page.getByRole('button', { name: 'New public link' }).click();
    await page.locator('#pub-new-port').fill(String(port));
    await page.locator('#pub-new-title').fill(title);
    await page.locator('#modal-btns').getByRole('button', { name: 'Create link' }).click();

    const row = page.locator('.pub-row', { hasText: title });
    await expect(row).toBeVisible();
    await expect(row).toContainText(`port ${port}`);
    const href = await row.locator('.pub-url').getAttribute('href');
    expect(href).toMatch(/\/pub\/[0-9a-f]{32}\/$/);

    // Anyone: a fresh context with no amux token or cookie.
    const base = new URL(page.url()).origin;
    const anon = await playwright.request.newContext({ ignoreHTTPSErrors: true, baseURL: base });
    const path = new URL(href!, base).pathname;
    const opened = await anon.get(path, { headers: { Cookie: 'amux_member=m; appsid=1' } });
    expect(opened.status()).toBe(200);
    const body = await opened.text();
    expect(body).toContain('<h1>shared app</h1>');
    expect(body).toContain('appsid=1');
    expect(body).not.toContain('amux_member');

    // The row fits a phone: title, link and both buttons on one row.
    const box = await row.boundingBox();
    expect(box!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    for (const name of ['Copy', 'Turn off']) {
      const b = await row.getByRole('button', { name }).boundingBox();
      expect(b!.height).toBeGreaterThanOrEqual(testInfo.project.name === 'desktop' ? 24 : 32);
    }

    await row.getByRole('button', { name: 'Turn off' }).click();
    await page.locator('#modal-btns').getByRole('button', { name: 'Turn off' }).click();
    await expect(row).toHaveCount(0);
    expect((await anon.get(path)).status()).toBe(404);
    await anon.dispose();
  } finally {
    app.close();
  }
});

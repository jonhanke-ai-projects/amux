// UI font picker (Ethan, 2026-10-14: "allow me to swap between: Geist, inter and
// IBM plex sans"). Settings > Device > Appearance > Font. The face must actually
// load (a silent fallback reads as a setting that does nothing), apply to
// controls as well as text, and survive a reload.
import { test, expect } from './fixtures';

test('the font picker loads each face, applies it to controls, and persists', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('amux_walkthrough_done', '1'));
  await page.goto('/');
  await page.waitForFunction(() => typeof (window as any).saveUiFont === 'function');
  await page.locator('#settings-btn').click();
  await page.locator('#settings-menu .settings-tab-btn[data-stab="device"]').click();
  const select = page.locator('#settings-font-select');
  await expect(select).toBeVisible();
  for (const [value, family] of [['geist', 'Geist'], ['inter', 'Inter'], ['plex', 'IBM Plex Sans']]) {
    await select.selectOption(value);
    const r = await page.evaluate(async (family) => {
      await document.fonts.load('400 14px "' + family + '"');
      return {
        loaded: document.fonts.check('400 14px "' + family + '"') && [...document.fonts].some(f => f.family.replace(/"/g, '') === family && f.status === 'loaded'),
        body: getComputedStyle(document.body).fontFamily,
        control: getComputedStyle(document.querySelector('#settings-font-select')!).fontFamily,
      };
    }, family);
    expect(r.loaded, family + ' loaded').toBe(true);
    expect(r.body.replace(/"/g, '')).toMatch(new RegExp('^' + family));
    expect(r.control.replace(/"/g, ''), 'controls inherit the font').toMatch(new RegExp('^' + family));
  }
  await page.reload();
  await page.waitForFunction(() => typeof (window as any).saveUiFont === 'function');
  expect(await page.evaluate(() => document.documentElement.dataset.font)).toBe('plex');
  await page.locator('#settings-btn').click();
  await page.locator('#settings-menu .settings-tab-btn[data-stab="device"]').click();
  await expect(page.locator('#settings-font-select')).toHaveValue('plex');
  await page.locator('#settings-font-select').selectOption('');
  expect(await page.evaluate(() => [document.documentElement.dataset.font ?? null, localStorage.getItem('amux_font')])).toEqual([null, null]);
});

import {test, expect} from './fixtures';
import {mkdtemp, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';

// A worker texts the owner a link to a markdown note to answer in place
// (Ethan, 2026-10-26: the self worker's 14-day check-in). #edit=<path> opens
// that file in Edit; #path=<path> keeps opening it in Preview.
test('#edit= opens a markdown file in Edit, #path= in Preview', async ({page}) => {
  const dir = await mkdtemp(join(tmpdir(), 'amux-edit-link-'));
  const md = join(dir, 'check in 2026-10-26.md');
  const txt = join(dir, 'notes.txt');
  await writeFile(md, '# Check-in\n\nHow did the last two weeks go?\n');
  await writeFile(txt, 'plain text\n');
  const enc = (p: string) => p.replace(/%/g, '%25').replace(/#/g, '%23').replace(/\?/g, '%3F').replace(/ /g, '%20');
  try {
    await page.goto('/#edit=' + enc(md));
    await expect(page.locator('#file-tab-edit')).toHaveClass(/active/, {timeout: 15000});
    await expect(page.locator('#file-save-btn')).toBeVisible();
    expect(await page.evaluate('_fileData && _fileData.path')).toBe(md);

    await page.goto('about:blank');
    await page.goto('/#path=' + enc(md));
    await expect(page.locator('#file-tab-preview')).toHaveClass(/active/, {timeout: 15000});
    await expect(page.locator('#file-tab-edit')).not.toHaveClass(/active/);

    // A file with no editor opens as it would from #path=.
    await page.goto('about:blank');
    await page.goto('/#edit=' + enc(txt));
    await expect.poll(() => page.evaluate('_fileData && _fileData.path'), {timeout: 15000}).toBe(txt);
    await expect(page.locator('#file-tab-edit')).not.toHaveClass(/active/);
  } finally {
    await rm(dir, {recursive: true, force: true});
  }
});

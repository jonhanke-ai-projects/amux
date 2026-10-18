// Big media over a disruptive connection (owner report 2026-10-12: a 106 MB
// .m4a in the iOS app sat at an hourglass and "0:00 / 0:00").
//
// Fixture: e2e/fixtures/media-fragmented.m4a, 20 s of a 440 Hz sine, mono AAC
// 16 kbps, fragmented every 1 s (ffmpeg -frag_duration 1000000 -movflags
// +dash+global_sidx), the same layout as the owner's file: ftyp, moov with
// mvex, sidx, then moof+mdat pairs. CI's images have no ffmpeg, so it is
// committed rather than generated.
//
// Every case runs on desktop, mobile and ios-safari. Decoding AAC needs a
// codec some CI browser builds lack; a case that needs decoding says so and
// skips by name rather than passing on a player that never played.
import {test, expect, Page} from './fixtures';
import {cleanup} from './teardown';
import {mkdtemp, copyFile, rm, readFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';

const FIXTURE = join(__dirname, 'fixtures', 'media-fragmented.m4a');

test.beforeEach(async ({page}) => {
  await page.route(/\/api\/sessions(?:\?.*)?$/, r => r.fulfill({json: []}));
});

async function withFixture(fn: (path: string) => Promise<void>) {
  const dir = await mkdtemp(join(tmpdir(), 'amux-media-'));
  try {
    const p = join(dir, 'talk one.m4a');
    await copyFile(FIXTURE, p);
    await fn(p);
  } finally { await cleanup('remove temp folder', () => rm(dir, {recursive: true, force: true})); }
}

const ev = (page: Page, expr: string) => page.evaluate(e => (0, eval)(e), expr);

async function canDecodeAac(page: Page) {
  return ev(page, `document.createElement('audio').canPlayType('audio/mp4; codecs="mp4a.40.2"')`);
}

async function openAndPlay(page: Page, path: string) {
  await page.evaluate(p => (window as any).openFilePreview(p), path);
  await expect(page.locator('#amux-audio-bar')).toHaveClass(/visible/);
  await expect.poll(() => ev(page, '_abAudio.currentTime'), {timeout: 20000, message: 'audio never advanced'}).toBeGreaterThan(1);
}

test('a fragmented MP4 gets a byte-range HLS playlist whose map and segments tile the file', async ({page}) => {
  await withFixture(async (path) => {
    await page.goto('/');
    const out = await page.evaluate(async (p) => {
      const meta = await (await fetch('/api/file?path=' + encodeURIComponent(p))).json();
      const r = await fetch(meta.hls_url);
      const m3u8 = await r.text();
      const base = new URL(meta.hls_url, location.href);
      const parts: {uri: string, len: number, off: number}[] = [];
      const map = m3u8.match(/#EXT-X-MAP:URI="([^"]+)",BYTERANGE="(\d+)@(\d+)"/);
      if (map) parts.push({uri: map[1], len: +map[2], off: +map[3]});
      const lines = m3u8.split('\n');
      lines.forEach((l, i) => {
        const b = l.match(/^#EXT-X-BYTERANGE:(\d+)@(\d+)$/);
        if (b) parts.push({uri: lines[i + 1], len: +b[1], off: +b[2]});
      });
      const statuses: number[] = [];
      const chunks: number[][] = [];
      for (const part of parts) {
        const res = await fetch(new URL(part.uri, base).href, {headers: {Range: `bytes=${part.off}-${part.off + part.len - 1}`}});
        statuses.push(res.status);
        chunks.push(Array.from(new Uint8Array(await res.arrayBuffer())));
      }
      return {meta, type: r.headers.get('content-type'), m3u8, statuses, chunks, parts,
        segments: parts.length - 1, ended: m3u8.trim().endsWith('#EXT-X-ENDLIST')};
    }, path);
    expect(out.meta.is_audio).toBe(true);
    expect(out.meta.stream).toEqual({hls_checked: true, fragmented: true});
    expect(out.type).toBe('application/vnd.apple.mpegurl');
    expect(out.ended).toBe(true);
    expect(out.segments, out.m3u8).toBeGreaterThanOrEqual(3);
    expect(new Set(out.statuses)).toEqual(new Set([206]));
    const file = await readFile(path);
    // Every fetched range is exactly those bytes of the file.
    out.parts.forEach((part, i) => {
      expect(Buffer.from(out.chunks[i]).equals(file.subarray(part.off, part.off + part.len)), `range ${i}`).toBe(true);
    });
    // The map starts the file and holds ftyp+moov; segments are contiguous to
    // the end; the only bytes in neither are index boxes (sidx, mfra).
    const [map, ...segs] = out.parts;
    expect(map.off).toBe(0);
    expect(file.subarray(4, 8).toString()).toBe('ftyp');
    segs.slice(1).forEach((sg, i) => expect(sg.off).toBe(segs[i].off + segs[i].len));
    // ...ending at EOF or at the trailing random-access index (mfra).
    const lastEnd = segs[segs.length - 1].off + segs[segs.length - 1].len;
    if (lastEnd !== file.length) expect(file.subarray(lastEnd + 4, lastEnd + 8).toString()).toBe('mfra');
    expect(file.subarray(segs[0].off + 4, segs[0].off + 8).toString()).toBe('moof');
    if (segs[0].off > map.len) expect(file.subarray(map.len + 4, map.len + 8).toString()).toBe('sidx');
  });
});

test('the bottom player reaches a finite duration and plays, on the engine this browser should use', async ({page}, info) => {
  await page.goto('/');
  test.skip(!(await canDecodeAac(page)), `${info.project.name}: this browser build cannot decode AAC, so playback is not measurable here`);
  await withFixture(async (path) => {
    await openAndPlay(page, path);
    const r = await ev(page, `({engine: _abKeeper.engine, d: _abAudio.duration, hls: _mediaPreferHls(_abAudio),
      dur: _abEls.dur.textContent, state: _abEls.state.textContent, src: _abAudio.getAttribute('src')})`);
    console.log(`[media] ${info.project.name}: ` + JSON.stringify(r));
    expect(Number.isFinite(r.d) && r.d > 15 && r.d < 25, 'duration ' + r.d).toBe(true);
    expect(r.dur).toMatch(/^0:(19|20|21)$/);
    expect(r.state).toBe('');
    // Apple WebKit takes the HLS playlist; everything else streams the file.
    expect(r.engine).toBe(r.hls ? 'hls' : 'raw');
    expect(r.src).toContain(r.engine === 'hls' ? '/api/file/hls?' : '/api/file/raw?');
  });
});

test('a fetch that fails while the network is down shows reconnecting, then resumes from the same position', async ({page}, info) => {
  await page.goto('/');
  test.skip(!(await canDecodeAac(page)), `${info.project.name}: this browser build cannot decode AAC, so resume is not measurable here`);
  await withFixture(async (path) => {
    let down = false;
    let blocked = 0;
    await page.route(/\/api\/file\/(raw|hls)\?/, r => {
      if (down) { blocked++; return r.abort('internetdisconnected'); }
      return r.continue();
    });
    await openAndPlay(page, path);
    await expect.poll(() => ev(page, '_abAudio.currentTime'), {timeout: 15000}).toBeGreaterThan(3);
    down = true;
    // The element refetches its source while the link is down (the fixture is
    // small enough to be fully buffered, so a drop alone would never be felt).
    // The first 'playing' after the drop is the resume point; recorded in the
    // page because the clip is short enough to finish before a poll sees it.
    const before: number = await ev(page, `(() => {
      window.__resumedAt = null;
      _abAudio.addEventListener('playing', () => { if (window.__resumedAt == null) window.__resumedAt = _abAudio.currentTime; });
      const t = _abAudio.currentTime; _abAudio.src = _abAudio.getAttribute('src'); return t; })()`);
    await expect(page.locator('#ab-state')).toHaveText(/Connection lost, retrying in \d+s|Reconnecting/, {timeout: 10000});
    await expect(page.locator('#amux-audio-bar')).toHaveAttribute('data-state', 'reconnecting');
    // An 'online' event retries at once instead of waiting out the backoff.
    await ev(page, `window.dispatchEvent(new Event('online'))`);
    await expect.poll(() => blocked, {timeout: 10000, message: 'no retry reached the network'}).toBeGreaterThan(1);
    down = false;
    await expect.poll(() => ev(page, `window.__resumedAt == null ? -1 : window.__resumedAt`),
      {timeout: 45000, message: 'playback never recovered'}).toBeGreaterThan(before - 1);
    const resumed: number = await ev(page, 'window.__resumedAt');
    expect(resumed, `resumed at ${resumed}s after dropping at ${before}s`).toBeGreaterThan(before - 1);
    expect(resumed).toBeLessThan(before + 15);
    await expect(page.locator('#ab-state')).toHaveText('');
    // Every recovery step reached the server log (two-fix rule).
    const beacons = await page.evaluate(async () =>
      (await (await fetch('/api/client-debug?kind=media-recovery&limit=50')).json()));
    const reasons = JSON.stringify(beacons);
    expect(reasons).toContain('"reason":"error"');
    expect(reasons).toContain('"reason":"online"');
    expect(reasons).toContain('"reason":"recovered"');
  });
});

test('the playback position survives a reload', async ({page}, info) => {
  await page.goto('/');
  test.skip(!(await canDecodeAac(page)), `${info.project.name}: this browser build cannot decode AAC, so position is not measurable here`);
  await withFixture(async (path) => {
    await openAndPlay(page, path);
    await expect.poll(() => ev(page, '_abAudio.currentTime'), {timeout: 15000}).toBeGreaterThan(4);
    const at: number = await ev(page, `(() => { _abKeeper.pause(); return _abAudio.currentTime; })()`);
    await page.reload();
    await page.waitForFunction(() => typeof (window as any).openFilePreview === 'function');
    await page.evaluate(p => (window as any).openFilePreview(p), path);
    await expect.poll(() => ev(page, '_abAudio.currentTime'), {timeout: 20000, message: 'did not resume'}).toBeGreaterThan(at - 1);
  });
});

test('closing the bar stops playback for good: no fetches, no auto-resume, and reopening resumes', async ({page}, info) => {
  await page.goto('/');
  const decodes = !!(await canDecodeAac(page));
  await withFixture(async (path) => {
    let media = 0;
    page.on('request', r => { if (/\/api\/file\/(raw|hls)\?/.test(r.url())) media++; });
    await page.evaluate(p => (window as any).openFilePreview(p), path);
    await expect(page.locator('#amux-audio-bar')).toHaveClass(/visible/);
    let at = 0;
    if (decodes) {
      await expect.poll(() => ev(page, '_abAudio.currentTime'), {timeout: 20000}).toBeGreaterThan(2);
      at = await ev(page, '_abAudio.currentTime');
    } else {
      console.log(`[media] ${info.project.name}: no AAC decoder; close is checked without playback`);
    }
    await page.evaluate(() => (window as any).closeFilePreview?.());
    await page.locator('#amux-audio-bar button[title="Close player"]').click();
    const closed = await ev(page, `({paused: _abAudio.paused, src: _abAudio.getAttribute('src'), net: _abAudio.networkState,
      visible: _abEls.bar.classList.contains('visible'), meta: 'mediaSession' in navigator ? navigator.mediaSession.metadata : null})`);
    expect(closed.paused).toBe(true);
    expect(closed.src).toBeNull();
    expect(closed.visible).toBe(false);
    expect(closed.meta).toBeNull();
    const after = media;
    // Nothing a reconnect path listens to may bring it back.
    await ev(page, `window.dispatchEvent(new Event('online')); document.dispatchEvent(new Event('visibilitychange'))`);
    await page.waitForTimeout(3000);
    expect(media, 'media requests after close').toBe(after);
    expect(await ev(page, '_abAudio.paused && !_abAudio.getAttribute("src")')).toBe(true);
    if (decodes) {
      await page.evaluate(p => (window as any).openFilePreview(p), path);
      await expect.poll(() => ev(page, '!_abAudio.paused && _abAudio.currentTime'), {timeout: 20000,
        message: 'reopening did not resume from the saved position'}).toBeGreaterThan(at - 1);
    }
  });
});

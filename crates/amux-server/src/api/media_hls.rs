//! HLS over a FRAGMENTED MP4 with no re-encode and no copy: a VOD media
//! playlist whose `EXT-X-MAP` is the file's init section (ftyp+moov) and whose
//! segments are `EXT-X-BYTERANGE` slices (runs of moof+mdat) of the SAME file,
//! all served by `GET /api/file/raw` (Range-capable). `GET /api/file/hls`
//! answers it; `GET /api/file` advertises it as `hls_url`.
//!
//! Why (2026-10-12, owner report from the iOS app): a 106 MB .m4a sat at an
//! hourglass and "0:00 / 0:00" in the bottom player. Measured in the iOS 18.7
//! Simulator: WebKit loads that fragmented file's metadata (duration 3459 s
//! after 863 ms) and then SUSPENDS at readyState 1; `canplay` never fires
//! until a user-initiated play(), and play() outside a gesture is
//! NotAllowedError. That hang was the player's own code (it waited for
//! `canplay`). HLS is the separate durability half: on a flaky phone link
//! AVFoundation retries a failed 1-2 MB segment and keeps the rest of its
//! buffer, where a single progressive byte stream is one long fetch that a
//! network change kills.
//!
//! Only fragmented files qualify: a classic MP4 has one moov describing one
//! mdat and cannot be sliced into independently decodable byte ranges without
//! remuxing. Those keep playing progressively from `raw_url`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

/// One moof(+mdat...) run: absolute byte span and its duration in seconds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fragment {
    pub start: u64,
    pub end: u64,
    pub secs: f64,
}

/// The parsed index of a fragmented MP4.
#[derive(Debug, Clone)]
pub(crate) struct Fmp4Index {
    /// Bytes [0, init_end) are ftyp..moov: the EXT-X-MAP init section.
    pub init_end: u64,
    pub fragments: Vec<Fragment>,
    /// Top-level boxes walked to build this (the `n_considered` population).
    pub boxes_scanned: usize,
}

/// Why a file has no HLS form. Every variant is a MEASURED answer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NotHls {
    /// Not an ISO-BMFF file, or the top-level box structure is broken.
    NotMp4(String),
    /// A classic (non-fragmented) MP4: moov has no mvex.
    NotFragmented,
    /// Fragmented per moov, but no usable moof was found.
    NoFragments,
}

impl NotHls {
    pub fn reason(&self) -> String {
        match self {
            NotHls::NotMp4(why) => format!("not an MP4 container: {why}"),
            NotHls::NotFragmented => {
                "not a fragmented MP4 (moov has no mvex); it plays progressively from raw_url".into()
            }
            NotHls::NoFragments => "moov declares fragments but no moof box was found".into(),
        }
    }
}

const MAX_BOX_READ: u64 = 64 << 20; // a moov/moof larger than this is not parsed

fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}
fn be_u64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8).map(|s| {
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        u64::from_be_bytes(a)
    })
}

/// Iterate child boxes inside an in-memory buffer: (type, payload start, end).
fn children(buf: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let Some(sz32) = be_u32(buf, off) else { break };
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&buf[off + 4..off + 8]);
        let (size, hdr) = match sz32 {
            0 => ((buf.len() - off) as u64, 8),
            1 => match be_u64(buf, off + 8) {
                Some(s) => (s, 16),
                None => break,
            },
            s => (s as u64, 8),
        };
        if size < hdr as u64 || off as u64 + size > buf.len() as u64 {
            break;
        }
        out.push((kind, off + hdr, off + size as usize));
        off += size as usize;
    }
    out
}

fn find<'a>(buf: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(buf)
        .into_iter()
        .find(|(k, _, _)| k == kind)
        .map(|(_, s, e)| &buf[s..e])
}

/// What the moov tells the fragment walk: per-track timescale and trex
/// default sample duration.
#[derive(Debug, Default)]
struct MoovInfo {
    fragmented: bool,
    timescale: HashMap<u32, u32>,
    trex_dur: HashMap<u32, u32>,
}

fn parse_moov(moov: &[u8]) -> MoovInfo {
    let mut info = MoovInfo::default();
    for (kind, s, e) in children(moov) {
        let body = &moov[s..e];
        match &kind {
            b"mvex" => {
                info.fragmented = true;
                for (k, ts, te) in children(body) {
                    if &k == b"trex" {
                        let t = &body[ts..te];
                        if let (Some(id), Some(dur)) = (be_u32(t, 4), be_u32(t, 12)) {
                            info.trex_dur.insert(id, dur);
                        }
                    }
                }
            }
            b"trak" => {
                let id = find(body, b"tkhd").and_then(|t| match t.first() {
                    Some(1) => be_u32(t, 20),
                    _ => be_u32(t, 12),
                });
                let scale = find(body, b"mdia")
                    .and_then(|m| find(m, b"mdhd"))
                    .and_then(|m| match m.first() {
                        Some(1) => be_u32(m, 20),
                        _ => be_u32(m, 12),
                    });
                if let (Some(id), Some(scale)) = (id, scale) {
                    if scale > 0 {
                        info.timescale.insert(id, scale);
                    }
                }
            }
            _ => {}
        }
    }
    info
}

/// Duration in seconds of one moof, measured on its FIRST traf's track.
fn moof_secs(moof: &[u8], info: &MoovInfo) -> Option<f64> {
    let traf = find(moof, b"traf")?;
    let tfhd = find(traf, b"tfhd")?;
    let flags = be_u32(tfhd, 0)? & 0x00FF_FFFF;
    let track = be_u32(tfhd, 4)?;
    let mut at = 8usize;
    if flags & 0x01 != 0 {
        at += 8;
    }
    if flags & 0x02 != 0 {
        at += 4;
    }
    let mut default_dur = info.trex_dur.get(&track).copied().unwrap_or(0);
    if flags & 0x08 != 0 {
        default_dur = be_u32(tfhd, at)?;
    }
    let mut ticks: u64 = 0;
    for (k, s, e) in children(traf) {
        if &k != b"trun" {
            continue;
        }
        let trun = &traf[s..e];
        let tf = be_u32(trun, 0)? & 0x00FF_FFFF;
        let count = be_u32(trun, 4)? as u64;
        let mut p = 8usize;
        if tf & 0x01 != 0 {
            p += 4;
        }
        if tf & 0x04 != 0 {
            p += 4;
        }
        if tf & 0x100 == 0 {
            ticks += count * default_dur as u64;
            continue;
        }
        let per = 4 * [0x100, 0x200, 0x400, 0x800].iter().filter(|f| tf & **f != 0).count();
        for i in 0..count as usize {
            ticks += be_u32(trun, p + i * per)? as u64;
        }
    }
    let scale = *info.timescale.get(&track)?;
    Some(ticks as f64 / scale as f64)
}

fn read_at(f: &mut File, off: u64, len: usize) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(off))?;
    let mut buf = vec![0u8; len];
    let mut got = 0;
    while got < len {
        match f.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    buf.truncate(got);
    Ok(buf)
}

/// Walk the file's top-level boxes. `full` false stops as soon as the moov is
/// known (the cheap probe `GET /api/file` runs on every media open).
fn walk(path: &Path, full: bool) -> Result<Fmp4Index, NotHls> {
    let mut f = File::open(path).map_err(|e| NotHls::NotMp4(e.to_string()))?;
    let size = f.metadata().map_err(|e| NotHls::NotMp4(e.to_string()))?.len();
    let mut off = 0u64;
    let mut scanned = 0usize;
    let mut moov: Option<(MoovInfo, u64)> = None;
    let mut frags: Vec<Fragment> = Vec::new();
    // A moof whose mdat never landed (a file still being written) is not a
    // playable segment; only fragments that gained an mdat are kept.
    let mut has_mdat: Vec<bool> = Vec::new();
    while off + 8 <= size {
        let hdr = read_at(&mut f, off, 16).map_err(|e| NotHls::NotMp4(e.to_string()))?;
        let sz32 = be_u32(&hdr, 0).unwrap_or(0);
        let kind: [u8; 4] = hdr[4..8].try_into().unwrap_or([0; 4]);
        if scanned == 0 && &kind != b"ftyp" && &kind != b"styp" {
            return Err(NotHls::NotMp4("first box is not ftyp".into()));
        }
        let (bsize, hlen) = match sz32 {
            0 => (size - off, 8u64),
            1 => (be_u64(&hdr, 8).unwrap_or(0), 16),
            s => (s as u64, 8),
        };
        if bsize < hlen || off + bsize > size {
            // A truncated tail (a file still being written) ends the walk; the
            // fragments before it are still valid.
            break;
        }
        scanned += 1;
        match &kind {
            b"moov" => {
                if bsize > MAX_BOX_READ {
                    return Err(NotHls::NotMp4("moov too large to index".into()));
                }
                let body = read_at(&mut f, off + hlen, (bsize - hlen) as usize)
                    .map_err(|e| NotHls::NotMp4(e.to_string()))?;
                let info = parse_moov(&body);
                if !info.fragmented {
                    return Err(NotHls::NotFragmented);
                }
                moov = Some((info, off + bsize));
                if !full {
                    break;
                }
            }
            b"moof" => {
                let Some((info, _)) = moov.as_ref() else {
                    return Err(NotHls::NotMp4("moof before moov".into()));
                };
                if bsize > MAX_BOX_READ {
                    break;
                }
                let body = read_at(&mut f, off + hlen, (bsize - hlen) as usize)
                    .map_err(|e| NotHls::NotMp4(e.to_string()))?;
                let secs = moof_secs(&body, info).unwrap_or(0.0);
                frags.push(Fragment { start: off, end: off + bsize, secs });
                has_mdat.push(false);
            }
            b"mdat" => {
                if let Some(last) = frags.last_mut() {
                    if last.end == off {
                        last.end = off + bsize;
                        if let Some(m) = has_mdat.last_mut() {
                            *m = true;
                        }
                    }
                }
            }
            _ => {}
        }
        off += bsize;
    }
    let mut keep = has_mdat.into_iter();
    frags.retain(|_| keep.next().unwrap_or(false));
    let Some((_, init_end)) = moov else {
        return Err(NotHls::NotMp4("no moov box".into()));
    };
    if full && frags.is_empty() {
        return Err(NotHls::NoFragments);
    }
    Ok(Fmp4Index { init_end, fragments: frags, boxes_scanned: scanned })
}

/// Cheap: is this a fragmented MP4? Reads box headers up to the moov only.
pub(crate) fn is_fragmented(path: &Path) -> bool {
    walk(path, false).is_ok()
}

type Key = (String, i64, u64);
fn cache() -> &'static Mutex<HashMap<Key, Arc<Fmp4Index>>> {
    static C: OnceLock<Mutex<HashMap<Key, Arc<Fmp4Index>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}
const CACHE_CAP: usize = 64;

/// The full index, cached per (path, mtime, size) so a rewrite re-indexes.
/// Returns (index, cache_hit).
pub(crate) fn index(path: &Path, mtime: i64, size: u64) -> Result<(Arc<Fmp4Index>, bool), NotHls> {
    let key = (path.to_string_lossy().into_owned(), mtime, size);
    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Ok((hit, true));
    }
    let idx = Arc::new(walk(path, true)?);
    if let Ok(mut c) = cache().lock() {
        if c.len() >= CACHE_CAP {
            c.clear();
        }
        c.insert(key, idx.clone());
    }
    Ok((idx, false))
}

/// Target segment length. Apple's authoring guide recommends 6 s.
pub(crate) fn segment_target_secs() -> f64 {
    std::env::var("AMUX_MEDIA_HLS_SEGMENT_S")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| *v >= 1.0 && *v <= 60.0)
        .unwrap_or(6.0)
}

/// Group byte-contiguous fragments into segments of about `target` seconds.
pub(crate) fn segments(frags: &[Fragment], target: f64) -> Vec<Fragment> {
    let mut out: Vec<Fragment> = Vec::new();
    for fr in frags {
        match out.last_mut() {
            Some(cur) if cur.end == fr.start && cur.secs < target => {
                cur.end = fr.end;
                cur.secs += fr.secs;
            }
            _ => out.push(fr.clone()),
        }
    }
    out
}

/// Render the VOD playlist. `seg_uri` is the (relative) raw URL every
/// segment and the init map point at.
pub(crate) fn playlist(idx: &Fmp4Index, segs: &[Fragment], seg_uri: &str) -> String {
    let max = segs.iter().map(|s| s.secs).fold(0.0f64, f64::max);
    let mut m = String::with_capacity(96 + segs.len() * (seg_uri.len() + 64));
    m.push_str("#EXTM3U\n#EXT-X-VERSION:7\n");
    m.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", max.ceil().max(1.0) as u64));
    m.push_str("#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n");
    m.push_str(&format!("#EXT-X-MAP:URI=\"{seg_uri}\",BYTERANGE=\"{}@0\"\n", idx.init_end));
    for s in segs {
        m.push_str(&format!(
            "#EXTINF:{:.6},\n#EXT-X-BYTERANGE:{}@{}\n{seg_uri}\n",
            s.secs,
            s.end - s.start,
            s.start
        ));
    }
    m.push_str("#EXT-X-ENDLIST\n");
    m
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }
    fn full(v: u8, flags: u32, rest: &[u8]) -> Vec<u8> {
        let mut b = vec![v];
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        b.extend_from_slice(rest);
        b
    }
    fn u32s(xs: &[u32]) -> Vec<u8> {
        xs.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    /// A synthetic fMP4: one track (id 1, timescale 1000), trex default 500,
    /// `n` fragments of `per` samples each. Odd fragments carry explicit
    /// per-sample durations in trun, even ones rely on the trex default.
    pub(crate) fn synth(n: usize, per: u32, fragmented: bool) -> Vec<u8> {
        let mut f = bx(b"ftyp", b"isomiso6");
        let tkhd = bx(b"tkhd", &full(0, 3, &u32s(&[0, 0, 1, 0, 0])));
        let mdhd = bx(b"mdhd", &full(0, 0, &u32s(&[0, 0, 1000, 0, 0])));
        let trak = bx(b"trak", &[tkhd, bx(b"mdia", &mdhd)].concat());
        let mut moov_body = bx(b"mvhd", &full(0, 0, &[0u8; 96]));
        if fragmented {
            let trex = bx(b"trex", &full(0, 0, &u32s(&[1, 1, 500, 0, 0])));
            moov_body.extend(bx(b"mvex", &trex));
        }
        moov_body.extend(trak);
        f.extend(bx(b"moov", &moov_body));
        for i in 0..n {
            let tfhd = bx(b"tfhd", &full(0, 0x020000, &u32s(&[1])));
            let trun = if i % 2 == 1 {
                let mut r = full(0, 0x100, &u32s(&[per]));
                r.extend(u32s(&vec![250; per as usize]));
                bx(b"trun", &r)
            } else {
                bx(b"trun", &full(0, 0, &u32s(&[per])))
            };
            f.extend(bx(b"moof", &bx(b"traf", &[tfhd, trun].concat())));
            f.extend(bx(b"mdat", &[0xAA; 100]));
        }
        f
    }

    fn tmp(bytes: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut t = tempfile::NamedTempFile::new().unwrap();
        t.write_all(bytes).unwrap();
        t
    }

    #[test]
    fn indexes_fragments_with_trex_and_trun_durations() {
        let t = tmp(&synth(4, 4, true));
        let idx = walk(t.path(), true).unwrap();
        assert_eq!(idx.fragments.len(), 4);
        // even: 4 x trex 500 = 2.0 s; odd: 4 x 250 = 1.0 s
        let secs: Vec<f64> = idx.fragments.iter().map(|f| f.secs).collect();
        assert_eq!(secs, vec![2.0, 1.0, 2.0, 1.0]);
        // init ends where the first moof starts, and fragments tile the file
        assert_eq!(idx.init_end, idx.fragments[0].start);
        for w in idx.fragments.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        let len = std::fs::metadata(t.path()).unwrap().len();
        assert_eq!(idx.fragments.last().unwrap().end, len);
        assert!(is_fragmented(t.path()));
    }

    #[test]
    fn classic_mp4_and_non_mp4_are_named_not_guessed() {
        let t = tmp(&synth(0, 0, false));
        assert_eq!(walk(t.path(), true).unwrap_err(), NotHls::NotFragmented);
        assert!(!is_fragmented(t.path()));
        let t = tmp(b"ID3\x03\x00 this is an mp3");
        assert!(matches!(walk(t.path(), true).unwrap_err(), NotHls::NotMp4(_)));
        let t = tmp(&synth(0, 0, true));
        assert_eq!(walk(t.path(), true).unwrap_err(), NotHls::NoFragments);
    }

    #[test]
    fn segments_group_contiguous_fragments_to_target_and_playlist_parses() {
        let t = tmp(&synth(6, 4, true));
        let idx = walk(t.path(), true).unwrap();
        let segs = segments(&idx.fragments, 3.0);
        // 2+1 | 2+1 | 2+1
        assert_eq!(segs.len(), 3);
        assert!(segs.iter().all(|s| (s.secs - 3.0).abs() < 1e-9));
        let total: u64 = segs.iter().map(|s| s.end - s.start).sum();
        assert_eq!(total + idx.init_end, std::fs::metadata(t.path()).unwrap().len());
        let m = playlist(&idx, &segs, "raw?path=/x.m4a");
        assert!(m.starts_with("#EXTM3U\n"));
        assert!(m.contains(&format!("#EXT-X-MAP:URI=\"raw?path=/x.m4a\",BYTERANGE=\"{}@0\"", idx.init_end)));
        assert_eq!(m.matches("#EXTINF:").count(), 3);
        assert_eq!(m.matches("#EXT-X-BYTERANGE:").count(), 3);
        assert!(m.contains("#EXT-X-TARGETDURATION:3\n"));
        assert!(m.trim_end().ends_with("#EXT-X-ENDLIST"));
    }

    #[test]
    fn truncated_tail_keeps_complete_fragments() {
        let mut b = synth(3, 4, true);
        b.truncate(b.len() - 50); // the last mdat is cut short
        let t = tmp(&b);
        let idx = walk(t.path(), true).unwrap();
        // the last moof's mdat never landed, so only two segments are playable
        assert_eq!(idx.fragments.len(), 2);
        assert!(idx.fragments.last().unwrap().end < b.len() as u64);
    }
}

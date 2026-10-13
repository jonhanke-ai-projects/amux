//! The firehose search index: session logs and transcripts (FTS5).
//!
//! `/api/search` indexes curated records (cards, memories, prompts) through
//! triggers in `amux.db`. Terminal logs and transcripts are a different shape:
//! files that only grow, 1.6 GB of them on the owner's machine (2026-10-13),
//! which `/api/log-search` used to grep end to end on every keystroke (1.6 s,
//! unranked, a substring match on the whole query). They do not belong in
//! `amux.db` either: it was already 15.8 GB, and this index is rebuildable from
//! the files at any time, so it lives in its own file that can simply be
//! deleted (`~/.amux/search-firehose.db`).
//!
//! INGEST IS INCREMENTAL, BY BYTE OFFSET. `runtime_jobs::search_ingest` calls
//! [`ingest`] every few seconds with a byte budget. Each source keeps a cursor
//! (inode, offset, line). A file that shrank or changed inode was rotated or
//! rewritten, so its documents are dropped and it is read again from zero.
//!
//! A QUERY IS NEVER STALE. The cursor says exactly which bytes the index has
//! not seen yet, so [`log_search`] scans only that unindexed tail directly.
//! While the first build runs, that tail is most of the file and the answer is
//! as slow as the old grep; once caught up it is a few KB per file.
//!
//! Logs are indexed in chunks of up to [`CHUNK_LINES`] cleaned lines, so a hit
//! can still be reported as `{line, text}` the way the log-search UI renders it.
//!
//! THE INDEX HOLDS NO TEXT. The first build stored each chunk's cleaned text
//! beside the FTS index and came to 1.75 GB for 0.99 GB of logs: 853 MB of it
//! was that second copy, 350 MB the index. The files are append-only and
//! already on disk, so a document is now a byte range (`start`, `end`) and the
//! FTS table is contentless. A hit re-reads its range and re-runs the same
//! deterministic cleaning, which yields the same lines and line numbers. A file
//! rewritten under the index is caught by the inode check and re-ingested.

use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Cleaned log lines per indexed chunk. Small enough that a chunk's bm25 rank
/// means "these lines are about the query", large enough that 1.6 GB of logs
/// is not tens of millions of rows.
pub const CHUNK_LINES: usize = 40;
/// One transcript entry is indexed up to this many chars.
const TRANSCRIPT_CHARS: usize = 16_000;
/// A matching line is returned up to this many chars (the log-search cap).
const LINE_CHARS: usize = 500;

pub fn db_path(home: &Path) -> PathBuf {
    home.join("search-firehose.db")
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS docs (
    id       INTEGER PRIMARY KEY,
    kind     TEXT NOT NULL,
    session  TEXT NOT NULL,
    source   TEXT NOT NULL,
    line     INTEGER NOT NULL,
    start    INTEGER NOT NULL,
    end      INTEGER NOT NULL,
    ts       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS docs_source  ON docs(source);
CREATE INDEX IF NOT EXISTS docs_session ON docs(kind, session);
CREATE VIRTUAL TABLE IF NOT EXISTS docs_fts USING fts5(
    body, content='', contentless_delete=1,
    tokenize='unicode61 remove_diacritics 2'
);
CREATE TABLE IF NOT EXISTS cursors (
    source  TEXT PRIMARY KEY,
    kind    TEXT NOT NULL,
    session TEXT NOT NULL,
    ino     INTEGER NOT NULL,
    offset  INTEGER NOT NULL,
    line    INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
";

/// Open (creating if needed) the index. WAL so the API reads while the ingest
/// job writes; a busy timeout so neither side errors on the other's lock.
pub fn open(home: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(db_path(home))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // The index is derived data: a file from another layout is dropped and
    // rebuilt by the next ingest pass, never migrated.
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        conn.execute_batch(
            "DROP TABLE IF EXISTS docs_fts; DROP TABLE IF EXISTS docs; DROP TABLE IF EXISTS cursors;",
        )?;
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        conn.execute_batch("VACUUM")?;
    } else {
        conn.execute_batch(SCHEMA)?;
    }
    Ok(conn)
}

/// Bumped whenever the tables change shape. 2: byte ranges, contentless FTS.
const SCHEMA_VERSION: i64 = 2;

/// Delete the index. The next ingest tick rebuilds it from the files.
pub fn reset(home: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{suffix}", db_path(home).display()));
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub kind: &'static str,
    pub session: String,
    pub path: PathBuf,
}

/// The session a log file belongs to: `amux.log` and its rotation `amux.log.1`
/// are both `amux`.
pub fn log_session(file_name: &str) -> Option<String> {
    let stem = file_name
        .strip_suffix(".log")
        .or_else(|| file_name.strip_suffix(".log.1"))?;
    (!stem.is_empty() && !stem.starts_with('.')).then(|| stem.to_string())
}

/// Every file the index covers: `logs/*.log`, `logs/*.log.1` and
/// `transcripts/<session>/*.jsonl`.
pub fn sources(home: &Path) -> Vec<Source> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(home.join("logs"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(session) = log_session(&name) {
            if e.path().is_file() {
                out.push(Source {
                    kind: "log",
                    session,
                    path: e.path(),
                });
            }
        }
    }
    for d in std::fs::read_dir(home.join("transcripts"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if !d.path().is_dir() {
            continue;
        }
        let session = d.file_name().to_string_lossy().into_owned();
        for f in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
            if f.path().extension().is_some_and(|x| x == "jsonl") {
                out.push(Source {
                    kind: "transcript",
                    session: session.clone(),
                    path: f.path(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

// ---------------------------------------------------------------------------
// Cleaning
// ---------------------------------------------------------------------------

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    // CSI sequences, OSC sequences (BEL or ST terminated), then any other
    // two-byte escape, then the remaining C0 controls except tab.
    Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]|[\x00-\x08\x0b-\x1f\x7f]")
        .unwrap()
});

/// One terminal line as a person would read it: escapes removed, and for a
/// carriage-return overwrite, the text that was left on screen.
pub fn clean_line(raw: &str) -> String {
    let visible = raw
        .rsplit('\r')
        .find(|seg| !seg.trim().is_empty())
        .unwrap_or("");
    ANSI.replace_all(visible, "").trim_end().to_string()
}

/// Lines with no letter or digit are box-drawing, spinners and rules: they
/// cannot match a query and only dilute ranking.
fn worth_indexing(line: &str) -> bool {
    line.chars().any(char::is_alphanumeric)
}

/// The searchable text of one transcript JSONL entry, or None for entries that
/// are not conversation (mode switches, titles, attachments).
pub fn transcript_text(entry: &Value) -> Option<String> {
    let role = entry.get("type").and_then(Value::as_str)?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let content = entry.get("message")?.get("content")?;
    let mut parts: Vec<String> = Vec::new();
    let mut push_block = |b: &Value| match b.get("type").and_then(Value::as_str) {
        Some("text") => {
            if let Some(t) = b.get("text").and_then(Value::as_str) {
                parts.push(t.to_string());
            }
        }
        Some("tool_result") => match b.get("content") {
            Some(Value::String(s)) => parts.push(s.clone()),
            Some(Value::Array(a)) => parts.extend(
                a.iter()
                    .filter_map(|x| x.get("text").and_then(Value::as_str))
                    .map(str::to_string),
            ),
            _ => {}
        },
        _ => {}
    };
    match content {
        Value::String(s) => parts.push(s.clone()),
        Value::Array(a) => a.iter().for_each(&mut push_block),
        _ => {}
    }
    let text = parts.join("\n");
    if !worth_indexing(&text) {
        return None;
    }
    Some(format!(
        "{role}: {}",
        text.chars().take(TRANSCRIPT_CHARS).collect::<String>()
    ))
}

/// One indexed log chunk: its line numbers, its cleaned text, and the byte
/// range in the slice it came from (first kept line's start to last kept
/// line's end, newline included).
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub nums: Vec<u64>,
    pub body: String,
    pub start: usize,
    pub end: usize,
}

/// Chunks of cleaned log lines. `first_line` is the 1-based number of the
/// first line in `bytes`. Repeats of the line just kept are dropped: a TUI
/// repaints the same status line thousands of times.
///
/// Byte offsets are taken on the raw bytes, never on a lossy UTF-8 copy, whose
/// replacement characters would shift every offset after an invalid byte.
/// Re-running this on one chunk's own range reproduces that chunk exactly,
/// which is what lets the index store ranges instead of text.
pub fn log_chunks(bytes: &[u8], first_line: u64) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut cur = Chunk {
        nums: vec![],
        body: String::new(),
        start: 0,
        end: 0,
    };
    let mut lines: Vec<String> = Vec::new();
    let mut last = String::new();
    let mut pos = 0usize;
    for (i, raw) in bytes.split(|b| *b == b'\n').enumerate() {
        let line_start = pos;
        pos += raw.len() + 1;
        let line = clean_line(&String::from_utf8_lossy(raw));
        if !worth_indexing(&line) || line == last {
            continue;
        }
        last = line.clone();
        if lines.is_empty() {
            cur.start = line_start;
        }
        cur.nums.push(first_line + i as u64);
        cur.end = pos.min(bytes.len());
        lines.push(line);
        if lines.len() >= CHUNK_LINES {
            cur.body = lines.join("\n");
            lines.clear();
            out.push(std::mem::replace(
                &mut cur,
                Chunk {
                    nums: vec![],
                    body: String::new(),
                    start: 0,
                    end: 0,
                },
            ));
        }
    }
    if !lines.is_empty() {
        cur.body = lines.join("\n");
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq)]
pub struct IngestReport {
    pub sources: usize,
    pub bytes: u64,
    pub docs: u64,
    pub reset: usize,
    pub pruned: usize,
}

struct Cursor {
    ino: u64,
    offset: u64,
    line: u64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Index up to `budget` new bytes across all sources, oldest-cursor first is
/// not needed: sources are visited in path order and each takes what remains.
pub fn ingest(home: &Path, budget: u64) -> rusqlite::Result<IngestReport> {
    let mut conn = open(home)?;
    let srcs = sources(home);
    let mut report = IngestReport {
        sources: srcs.len(),
        ..Default::default()
    };

    // Files that disappeared take their documents with them.
    let known: Vec<String> = conn
        .prepare("SELECT source FROM cursors")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for gone in known
        .iter()
        .filter(|k| !srcs.iter().any(|s| s.path.to_string_lossy() == k.as_str()))
    {
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM docs_fts WHERE rowid IN (SELECT id FROM docs WHERE source = ?1)",
            [gone],
        )?;
        tx.execute("DELETE FROM docs WHERE source = ?1", [gone])?;
        tx.execute("DELETE FROM cursors WHERE source = ?1", [gone])?;
        tx.commit()?;
        report.pruned += 1;
    }

    let mut left = budget;
    for src in &srcs {
        if left == 0 {
            break;
        }
        let Ok(meta) = std::fs::metadata(&src.path) else {
            continue;
        };
        let key = src.path.to_string_lossy().into_owned();
        let cur: Option<Cursor> = conn
            .query_row(
                "SELECT ino, offset, line FROM cursors WHERE source = ?1",
                [&key],
                |r| {
                    Ok(Cursor {
                        ino: r.get::<_, i64>(0)? as u64,
                        offset: r.get::<_, i64>(1)? as u64,
                        line: r.get::<_, i64>(2)? as u64,
                    })
                },
            )
            .optional()?;
        let mut cur = cur.unwrap_or(Cursor {
            ino: meta.ino(),
            offset: 0,
            line: 0,
        });
        if cur.ino != meta.ino() || meta.len() < cur.offset {
            conn.execute(
                "DELETE FROM docs_fts WHERE rowid IN (SELECT id FROM docs WHERE source = ?1)",
                [&key],
            )?;
            conn.execute("DELETE FROM docs WHERE source = ?1", [&key])?;
            cur = Cursor {
                ino: meta.ino(),
                offset: 0,
                line: 0,
            };
            report.reset += 1;
        }
        if meta.len() == cur.offset {
            continue;
        }
        let want = (meta.len() - cur.offset).min(left);
        let mut buf = Vec::with_capacity(want as usize);
        let Ok(mut f) = std::fs::File::open(&src.path) else {
            continue;
        };
        if f.seek(SeekFrom::Start(cur.offset)).is_err()
            || (&mut f).take(want).read_to_end(&mut buf).is_err()
        {
            continue;
        }
        // Only whole lines: a partial last line waits for its newline. A single
        // line longer than the budget is taken whole so it cannot stall forever.
        let end = match buf.iter().rposition(|b| *b == b'\n') {
            Some(i) => i + 1,
            None if want < meta.len() - cur.offset => {
                let mut rest = Vec::new();
                if f.read_to_end(&mut rest).is_err() {
                    continue;
                }
                match rest.iter().position(|b| *b == b'\n') {
                    Some(i) => {
                        buf.extend_from_slice(&rest[..=i]);
                        buf.len()
                    }
                    None => continue,
                }
            }
            None => continue,
        };
        let chunk_bytes = &buf[..end];
        let ts = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or_else(now_ms);
        let tx = conn.transaction()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO docs (kind, session, source, line, start, end, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut fts =
                tx.prepare_cached("INSERT INTO docs_fts (rowid, body) VALUES (?1, ?2)")?;
            let base = cur.offset as i64;
            let mut put =
                |line: i64, start: usize, end: usize, body: &str| -> rusqlite::Result<()> {
                    ins.execute(params![
                        src.kind,
                        src.session,
                        key,
                        line,
                        base + start as i64,
                        base + end as i64,
                        ts
                    ])?;
                    fts.execute(params![tx.last_insert_rowid(), body])?;
                    report.docs += 1;
                    Ok(())
                };
            match src.kind {
                "log" => {
                    for c in log_chunks(chunk_bytes, cur.line + 1) {
                        put(c.nums[0] as i64, c.start, c.end, &c.body)?;
                    }
                }
                _ => {
                    let mut pos = 0usize;
                    for (i, raw) in chunk_bytes.split(|b| *b == b'\n').enumerate() {
                        let start = pos;
                        pos += raw.len() + 1;
                        let Ok(v) = serde_json::from_slice::<Value>(raw) else {
                            continue;
                        };
                        if let Some(body) = transcript_text(&v) {
                            put(
                                (cur.line + 1 + i as u64) as i64,
                                start,
                                start + raw.len(),
                                &body,
                            )?;
                        }
                    }
                }
            }
        }
        let lines_read = chunk_bytes.iter().filter(|b| **b == b'\n').count() as u64;
        tx.execute(
            "INSERT INTO cursors (source, kind, session, ino, offset, line, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(source) DO UPDATE SET ino=excluded.ino, offset=excluded.offset,
                 line=excluded.line, updated=excluded.updated",
            params![
                key,
                src.kind,
                src.session,
                cur.ino as i64,
                (cur.offset + end as u64) as i64,
                (cur.line + lines_read) as i64,
                now_ms()
            ],
        )?;
        tx.commit()?;
        report.bytes += end as u64;
        left = left.saturating_sub(end as u64);
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

/// The lowercased words of a query, for picking the matching LINES out of a
/// matching chunk. Mirrors the tokenizer closely enough for that job: split on
/// anything that is not a letter or digit.
pub fn query_words(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn truncate(s: &str) -> String {
    s.chars().take(LINE_CHARS).collect()
}

/// The (line number, text) pairs of a chunk that contain at least one query
/// word. A chunk matches when ALL words occur somewhere in it, so each line is
/// shown if it carries any of them.
pub fn matching_lines(body: &str, nums: &[u64], words: &[String]) -> Vec<(u64, String)> {
    body.split('\n')
        .zip(nums.iter().copied())
        .filter(|(l, _)| {
            let ll = l.to_lowercase();
            words.iter().any(|w| ll.contains(w.as_str()))
        })
        .map(|(l, n)| (n, truncate(l)))
        .collect()
}

/// Re-reads indexed ranges from their files, one open handle per file.
#[derive(Default)]
struct RangeReader {
    files: std::collections::HashMap<String, Option<std::fs::File>>,
}

impl RangeReader {
    fn bytes(&mut self, source: &str, start: i64, end: i64) -> Option<Vec<u8>> {
        let f = self
            .files
            .entry(source.to_string())
            .or_insert_with(|| std::fs::File::open(source).ok())
            .as_mut()?;
        let len = (end - start).max(0) as usize;
        let mut buf = vec![0u8; len];
        f.seek(SeekFrom::Start(start as u64)).ok()?;
        f.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    /// The document's line numbers and text, rebuilt from its range.
    fn load(
        &mut self,
        kind: &str,
        source: &str,
        line: i64,
        start: i64,
        end: i64,
    ) -> Option<(Vec<u64>, String)> {
        let bytes = self.bytes(source, start, end)?;
        if kind == "log" {
            let c = log_chunks(&bytes, line as u64).into_iter().next()?;
            Some((c.nums, c.body))
        } else {
            let v: Value = serde_json::from_slice(&bytes).ok()?;
            transcript_text(&v).map(|b| (vec![line as u64], b))
        }
    }
}

/// A case-insensitive matcher for the query words, longest first so a word
/// that contains another is highlighted whole.
fn word_regex(words: &[String]) -> Option<Regex> {
    let mut w: Vec<&String> = words.iter().collect();
    w.sort_by_key(|x| std::cmp::Reverse(x.len()));
    if w.is_empty() {
        return None;
    }
    Regex::new(&format!(
        "(?i){}",
        w.iter()
            .map(|x| regex::escape(x))
            .collect::<Vec<_>>()
            .join("|")
    ))
    .ok()
}

/// A short excerpt around the first match, with each match wrapped in the
/// \u{1}..\u{2} sentinels `api::search::render_snippet` turns into <mark>.
/// Contentless FTS5 cannot run snippet(), so this stands in for it.
pub fn make_snippet(body: &str, re: Option<&Regex>) -> String {
    const BEFORE: usize = 80;
    const WIDTH: usize = 280;
    let first = re
        .and_then(|r| r.find(body))
        .map(|m| m.start())
        .unwrap_or(0);
    let start_char = body[..first].chars().count().saturating_sub(BEFORE);
    let excerpt: String = body.chars().skip(start_char).take(WIDTH).collect();
    let mut out = String::new();
    if start_char > 0 {
        out.push('…');
    }
    match re {
        Some(r) => out.push_str(&r.replace_all(&excerpt, "\u{1}$0\u{2}")),
        None => out.push_str(&excerpt),
    }
    if body.chars().count() > start_char + WIDTH {
        out.push('…');
    }
    out
}

/// Ranked hits for `/api/search`, in its hit shape. `kinds` is a subset of
/// `log`/`transcript`; `session` optionally scopes to one session.
pub fn search(
    home: &Path,
    match_expr: &str,
    q: &str,
    kinds: &[&str],
    session: Option<&str>,
    limit: usize,
    count_cap: usize,
) -> rusqlite::Result<(Vec<Value>, usize, bool)> {
    if kinds.is_empty() || !db_path(home).exists() {
        return Ok((vec![], 0, false));
    }
    let conn = open(home)?;
    let kind_list = kinds
        .iter()
        .map(|k| format!("'{}'", k.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    let sess = if session.is_some() {
        " AND d.session = ?3"
    } else {
        ""
    };
    let sql = format!(
        "SELECT d.id, d.kind, d.session, d.source, d.line, d.ts, d.start, d.end,
                bm25(docs_fts) AS rank
         FROM docs_fts JOIN docs d ON d.id = docs_fts.rowid
         WHERE docs_fts MATCH ?1 AND d.kind IN ({kind_list}){sess}
         ORDER BY rank LIMIT ?2"
    );
    let mut st = conn.prepare(&sql)?;
    let words = query_words(q);
    let re = word_regex(&words);
    let reader = std::cell::RefCell::new(RangeReader::default());
    let map = |r: &rusqlite::Row| -> rusqlite::Result<Value> {
        let kind: String = r.get(1)?;
        let session: String = r.get(2)?;
        let source: String = r.get(3)?;
        let line: i64 = r.get(4)?;
        let loaded = reader
            .borrow_mut()
            .load(&kind, &source, line, r.get(6)?, r.get(7)?);
        let snippet = loaded
            .as_ref()
            .map(|(_, body)| make_snippet(body, re.as_ref()))
            .unwrap_or_default();
        // Name the first line that carries a query word, not the chunk's first
        // line: "log:1" for a match on line 2 sends the reader to the wrong place.
        let line = loaded
            .as_ref()
            .and_then(|(nums, body)| {
                matching_lines(body, nums, &words)
                    .first()
                    .map(|(n, _)| *n as i64)
            })
            .unwrap_or(line);
        let file = Path::new(&source)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(json!({
            "doc_id": format!("{kind}:{}", r.get::<_, i64>(0)?),
            "type": kind,
            "id": format!("{file}:{line}"),
            "title": format!("{session} · {file} line {line}"),
            "scope": session,
            "task_id": Value::Null,
            "worker_id": session,
            "link": format!("#peek/{session}"),
            "meta": {"session": session, "source": source, "line": line},
            "updated_at": r.get::<_, i64>(5)?,
            "snippet": snippet,
            "rank": r.get::<_, f64>(8)?,
        }))
    };
    let hits: Vec<Value> = match session {
        Some(s) => st
            .query_map(params![match_expr, limit as i64, s], map)?
            .collect::<rusqlite::Result<_>>()?,
        None => st
            .query_map(params![match_expr, limit as i64], map)?
            .collect::<rusqlite::Result<_>>()?,
    };
    let count_sql = format!(
        "SELECT COUNT(*) FROM (SELECT 1 FROM docs_fts JOIN docs d ON d.id = docs_fts.rowid
         WHERE docs_fts MATCH ?1 AND d.kind IN ({kind_list}){sess} LIMIT ?2)"
    );
    let total: i64 = match session {
        Some(s) => conn.query_row(
            &count_sql,
            params![match_expr, (count_cap + 1) as i64, s],
            |r| r.get(0),
        )?,
        None => conn.query_row(
            &count_sql,
            params![match_expr, (count_cap + 1) as i64],
            |r| r.get(0),
        )?,
    };
    Ok((
        hits,
        (total as usize).min(count_cap),
        total as usize > count_cap,
    ))
}

/// `/api/log-search`: `{session: [{line, text}]}`, up to `max_per` lines per
/// session, from the index plus a direct scan of each file's unindexed tail.
/// Returns the matches and how many tail bytes had to be scanned.
pub fn log_search(
    home: &Path,
    match_expr: &str,
    q: &str,
    max_per: usize,
) -> rusqlite::Result<(BTreeMap<String, Vec<Value>>, u64)> {
    let words = query_words(q);
    let mut out: BTreeMap<String, Vec<(String, u64, String)>> = BTreeMap::new();
    let conn = open(home)?;
    {
        let mut st = conn.prepare(
            "SELECT d.session, d.source, d.line, d.start, d.end
             FROM docs_fts JOIN docs d ON d.id = docs_fts.rowid
             WHERE docs_fts MATCH ?1 AND d.kind = 'log'
             ORDER BY d.source, d.line",
        )?;
        let rows = st.query_map([match_expr], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut reader = RangeReader::default();
        for row in rows {
            let (session, source, line, start, end) = row?;
            let bucket = out.entry(session).or_default();
            if bucket.len() >= max_per {
                continue;
            }
            let Some((nums, body)) = reader.load("log", &source, line, start, end) else {
                continue;
            };
            for (n, text) in matching_lines(&body, &nums, &words) {
                if bucket.len() >= max_per {
                    break;
                }
                bucket.push((source.clone(), n, text));
            }
        }
    }
    // The bytes the index has not reached yet, read straight from the file.
    let mut tail_bytes = 0u64;
    for src in sources(home).into_iter().filter(|s| s.kind == "log") {
        let key = src.path.to_string_lossy().into_owned();
        let Ok(meta) = std::fs::metadata(&src.path) else {
            continue;
        };
        let cur: Option<(i64, i64, i64)> = conn
            .query_row(
                "SELECT ino, offset, line FROM cursors WHERE source = ?1",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (offset, line) = match cur {
            Some((ino, off, line)) if ino as u64 == meta.ino() && off as u64 <= meta.len() => {
                (off as u64, line as u64)
            }
            _ => (0, 0),
        };
        if offset >= meta.len() {
            continue;
        }
        let bucket = out.entry(src.session.clone()).or_default();
        if bucket.len() >= max_per {
            continue;
        }
        let Ok(mut f) = std::fs::File::open(&src.path) else {
            continue;
        };
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(offset)).is_err() || f.read_to_end(&mut buf).is_err() {
            continue;
        }
        tail_bytes += buf.len() as u64;
        let text = String::from_utf8_lossy(&buf);
        for (i, raw) in text.split('\n').enumerate() {
            // Cheap test on the raw bytes first; only a candidate pays for the
            // escape-stripping regex. Cleaning every line made this tail scan
            // 6x slower than the grep it replaces (measured mid-build, 793 MB).
            let raw_lc = raw.to_lowercase();
            if words.is_empty() || !words.iter().all(|w| raw_lc.contains(w.as_str())) {
                continue;
            }
            let clean = clean_line(raw);
            let lc = clean.to_lowercase();
            if words.iter().all(|w| lc.contains(w.as_str())) {
                bucket.push((key.clone(), line + 1 + i as u64, truncate(&clean)));
                if bucket.len() >= max_per {
                    break;
                }
            }
        }
    }
    let matches = out
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(s, v)| {
            let multi_file = v.iter().any(|(src, ..)| src != &v[0].0);
            let hits = v
                .into_iter()
                .map(|(src, n, text)| {
                    let mut h = json!({"line": n, "text": text});
                    if multi_file {
                        h["file"] = json!(Path::new(&src)
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned()));
                    }
                    h
                })
                .collect();
            (s, hits)
        })
        .collect();
    Ok((matches, tail_bytes))
}

/// Bytes past the cursor that the next ingest pass can take: through the last
/// newline. A trailing partial line (a shell prompt waiting for input) cannot
/// be indexed yet, and counting it would keep `caught_up` false forever on any
/// idle worker. Past 1 MB the exact answer does not matter: it is behind.
fn indexable_lag(path: &Path, offset: u64, len: u64) -> u64 {
    let rest = len.saturating_sub(offset);
    if rest == 0 || rest > 1 << 20 {
        return rest;
    }
    let mut buf = Vec::with_capacity(rest as usize);
    let read = std::fs::File::open(path).and_then(|mut f| {
        f.seek(SeekFrom::Start(offset))?;
        f.take(rest).read_to_end(&mut buf)
    });
    match read {
        Ok(_) => buf
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| i as u64 + 1)
            .unwrap_or(0),
        Err(_) => rest,
    }
}

/// Coverage for `/api/search/status`: per kind, documents and sources, and how
/// many bytes are still waiting to be indexed.
pub fn status(home: &Path) -> Value {
    let srcs = sources(home);
    let conn = match open(home) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };
    let mut kinds = Vec::new();
    let mut lag_total = 0u64;
    for kind in ["log", "transcript"] {
        let docs: i64 = conn
            .query_row("SELECT COUNT(*) FROM docs WHERE kind = ?1", [kind], |r| {
                r.get(0)
            })
            .unwrap_or(-1);
        let mut size = 0u64;
        let mut lag = 0u64;
        let mut n = 0usize;
        for s in srcs.iter().filter(|s| s.kind == kind) {
            let Ok(meta) = std::fs::metadata(&s.path) else {
                continue;
            };
            n += 1;
            size += meta.len();
            let off: u64 = conn
                .query_row(
                    "SELECT offset FROM cursors WHERE source = ?1 AND ino = ?2",
                    params![s.path.to_string_lossy(), meta.ino() as i64],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .ok()
                .flatten()
                .map(|o| o as u64)
                .unwrap_or(0);
            lag += indexable_lag(&s.path, off, meta.len());
        }
        lag_total += lag;
        kinds.push(json!({
            "type": kind,
            "docs": docs,
            "sources": n,
            "source_bytes": size,
            "lag_bytes": lag,
        }));
    }
    json!({
        "path": db_path(home),
        "db_bytes": std::fs::metadata(db_path(home)).map(|m| m.len()).unwrap_or(0),
        "caught_up": lag_total == 0,
        "lag_bytes": lag_total,
        "kinds": kinds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn match_of(q: &str) -> String {
        crate::api::search::build_match(q).unwrap()
    }

    #[test]
    fn cleaning_strips_escapes_and_keeps_what_was_left_on_screen() {
        assert_eq!(clean_line("\x1b[1;32mok\x1b[0m done"), "ok done");
        assert_eq!(clean_line("\x1b]0;title\x07hello"), "hello");
        assert_eq!(clean_line("spinner 1\rspinner 2\r"), "spinner 2");
        assert!(!worth_indexing("──────╭╮"));
    }

    #[test]
    fn chunks_keep_real_line_numbers_and_drop_repaints() {
        let text = "x\nalpha\n\n───\nbeta\nbeta\ngamma\n";
        let c = log_chunks(&text.as_bytes()[2..], 10);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].nums, vec![10, 13, 15]);
        assert_eq!(c[0].body, "alpha\nbeta\ngamma");
        // The range reproduces the chunk on its own, which is what lets the
        // index store ranges instead of text.
        let sub = &text.as_bytes()[2..][c[0].start..c[0].end];
        assert_eq!(
            log_chunks(sub, 10),
            vec![Chunk {
                start: 0,
                end: sub.len(),
                ..c[0].clone()
            }]
        );
        // Invalid UTF-8 does not shift offsets.
        let raw = b"\xff\xfebad\nnext line\n";
        let c = log_chunks(raw, 1);
        assert_eq!(&raw[c[0].start..c[0].end], &raw[..]);
    }

    #[test]
    fn log_session_names_rotations_and_refuses_junk() {
        assert_eq!(log_session("amux.log").as_deref(), Some("amux"));
        assert_eq!(log_session("amux.log.1").as_deref(), Some("amux"));
        assert_eq!(log_session("x.jsonl"), None);
        assert_eq!(log_session(".log"), None);
    }

    #[test]
    fn transcript_text_reads_messages_and_skips_bookkeeping() {
        let v = json!({"type":"assistant","message":{"content":[
            {"type":"thinking","thinking":"hidden"},
            {"type":"text","text":"The reaper closes idle Chrome"}]}});
        assert_eq!(
            transcript_text(&v).as_deref(),
            Some("assistant: The reaper closes idle Chrome")
        );
        assert_eq!(transcript_text(&json!({"type":"mode","mode":"x"})), None);
        let s = json!({"type":"user","message":{"content":"plain prompt"}});
        assert_eq!(transcript_text(&s).as_deref(), Some("user: plain prompt"));
    }

    /// The whole path on real files: ingest, rank, line hits, the unindexed
    /// tail, rotation and pruning.
    #[test]
    fn ingest_then_search_end_to_end() {
        let home = tempfile::tempdir().unwrap();
        let logs = home.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join("alpha.log"),
            "booting\n\x1b[31mbrowser\x1b[0m started\nthe routing driver failed\nunrelated\n",
        )
        .unwrap();
        std::fs::write(logs.join("beta.log"), "routing only here\n").unwrap();
        let tdir = home.path().join("transcripts").join("alpha");
        std::fs::create_dir_all(&tdir).unwrap();
        std::fs::write(
            tdir.join("t.jsonl"),
            "{\"type\":\"user\",\"message\":{\"content\":\"why is browser routing slow\"}}\n",
        )
        .unwrap();

        let r = ingest(home.path(), 1 << 20).unwrap();
        assert_eq!(r.sources, 3);
        assert_eq!(status(home.path())["caught_up"], json!(true));

        // AND across a chunk: both words, not adjacent, in alpha only.
        let m = match_of("browser routing");
        let (hits, total, _) =
            search(home.path(), &m, "browser routing", &["log"], None, 10, 100).unwrap();
        assert_eq!(total, 1);
        assert_eq!(hits[0]["scope"], json!("alpha"));
        assert_eq!(
            hits[0]["id"],
            json!("alpha.log:2"),
            "first matching line, not chunk start"
        );
        // The snippet is rebuilt from the file, cleaned, with both words marked.
        assert_eq!(
            hits[0]["snippet"],
            json!("booting\n\u{1}browser\u{2} started\nthe \u{1}routing\u{2} driver failed\nunrelated")
        );
        let (hits, ..) = search(
            home.path(),
            &m,
            "browser routing",
            &["transcript"],
            None,
            10,
            100,
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        let (hits, ..) = search(
            home.path(),
            &m,
            "browser routing",
            &["log"],
            Some("beta"),
            10,
            100,
        )
        .unwrap();
        assert!(hits.is_empty(), "session scope holds");

        let (lines, tail) = log_search(home.path(), &m, "browser routing", 50).unwrap();
        assert_eq!(tail, 0, "caught up: nothing scanned directly");
        assert_eq!(
            json!(lines["alpha"]),
            json!([{"line":2,"text":"browser started"},{"line":3,"text":"the routing driver failed"}])
        );

        // Appended but not yet ingested: found through the tail scan.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(logs.join("beta.log"))
            .unwrap();
        std::io::Write::write_all(&mut f, b"browser routing appended\n").unwrap();
        let (lines, tail) = log_search(home.path(), &m, "browser routing", 50).unwrap();
        assert!(tail > 0);
        assert_eq!(
            json!(lines["beta"]),
            json!([{"line":2,"text":"browser routing appended"}])
        );
        ingest(home.path(), 1 << 20).unwrap();
        let (lines, tail) = log_search(home.path(), &m, "browser routing", 50).unwrap();
        assert_eq!(tail, 0);
        assert_eq!(
            json!(lines["beta"]),
            json!([{"line":2,"text":"browser routing appended"}])
        );

        // Rewritten smaller: its old documents go, the new text is indexed.
        std::fs::write(logs.join("alpha.log"), "fresh\n").unwrap();
        let r = ingest(home.path(), 1 << 20).unwrap();
        assert_eq!(r.reset, 1);
        let (hits, ..) = search(
            home.path(),
            &m,
            "browser routing",
            &["log"],
            Some("alpha"),
            10,
            100,
        )
        .unwrap();
        assert!(hits.is_empty());

        // Deleted: pruned.
        std::fs::remove_file(logs.join("beta.log")).unwrap();
        assert_eq!(ingest(home.path(), 1 << 20).unwrap().pruned, 1);
        let (hits, ..) =
            search(home.path(), &m, "browser routing", &["log"], None, 10, 100).unwrap();
        assert!(hits.is_empty());
    }

    /// A partial last line waits for its newline; the budget is respected.
    #[test]
    fn ingest_takes_whole_lines_within_budget() {
        let home = tempfile::tempdir().unwrap();
        let logs = home.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("a.log"), "one line\ntwo line\npartial").unwrap();
        let r = ingest(home.path(), 12).unwrap();
        assert_eq!(r.bytes, 9, "stops at the last newline inside the budget");
        let r = ingest(home.path(), 1 << 20).unwrap();
        assert_eq!(r.bytes, 9, "the partial line is not taken");
        assert_eq!(
            status(home.path())["lag_bytes"],
            json!(0),
            "a partial line is not lag"
        );
        assert_eq!(status(home.path())["caught_up"], json!(true));
    }
}

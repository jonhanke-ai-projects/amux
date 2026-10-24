//! /api/sql — the SQL tab: schema browse, row browse, and query execution.
//!
//! Never ported; the tab is visible by default, so clicking it showed a dead
//! panel (route census, AMUX-2871).
//!
//! THE READ/WRITE SPLIT IS ENFORCED BY SQLITE, NOT BY READING THE SQL.
//! The client sends `{sql, write}` and a naive port would gate on a prefix
//! check — but `SELECT 1; DROP TABLE issues` passes any prefix check, and a
//! blocklist of keywords is a guessing game whose failure mode is silent data
//! loss on the live board. Instead a non-write query runs on a connection
//! opened SQLITE_OPEN_READ_ONLY: the engine refuses the write, so the guard
//! cannot be out-thought by a query shape nobody predicted. That is the
//! structurally-absent signal the ethos file prefers over a tuned parameter.

use super::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::types::ValueRef;
use serde_json::{json, Value};
use std::time::Instant;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", axum::routing::post(run_sql))
        .route("/ask", axum::routing::post(ask_sql))
        .route("/schema", get(schema))
        .route("/rows", get(rows))
}

fn db_path() -> std::path::PathBuf {
    std::env::var("AMUX_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| crate::api::session_verbs::home().join("amux.db"))
}

/// Cell -> JSON. NULL stays null rather than becoming "" — a browser that
/// cannot tell an empty string from a NULL is lying about the data it exists
/// to show.
fn cell(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!(format!("<{} bytes>", b.len())),
    }
}

const SQL_ROW_CAP: usize = 1000;

fn rows_to_json(
    stmt: &mut rusqlite::Statement<'_>,
    cap: usize,
) -> rusqlite::Result<(Vec<String>, Vec<Vec<Value>>, bool)> {
    let cols: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
    let mut out = Vec::new();
    let mut q = stmt.query([])?;
    while let Some(r) = q.next()? {
        if out.len() == cap {
            return Ok((cols, out, true));
        }
        let mut values = Vec::with_capacity(cols.len());
        for i in 0..cols.len() {
            values.push(cell(r.get_ref(i)?));
        }
        out.push(values);
    }
    Ok((cols, out, false))
}

async fn schema(State(state): State<AppState>) -> Response {
    let conn = match state.store.read() {
        Ok(c) => c,
        Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, e.to_string()).into_response(),
    };
    let mut tables = Vec::new();
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0)).map(|r| r.flatten().collect()))
        .unwrap_or_default();
    for t in names {
        let cols: Vec<Value> = conn
            .prepare(&format!(
                "PRAGMA table_info(\"{}\")",
                t.replace('"', "\"\"")
            ))
            .and_then(|mut s| {
                s.query_map([], |r| {
                    Ok(json!({
                        "name": r.get::<_, String>(1)?,
                        "type": r.get::<_, String>(2)?,
                        "notnull": r.get::<_, i64>(3)? == 1,
                        "dflt": r.get::<_, Option<String>>(4)?,
                        "pk": r.get::<_, i64>(5)? > 0,
                    }))
                })
                .map(|r| r.flatten().collect())
            })
            .unwrap_or_default();
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM \"{}\"", t.replace('"', "\"\"")),
                [],
                |r| r.get(0),
            )
            .unwrap_or(-1);
        tables.push(json!({
            "name": t,
            "columns": cols,
            "rows": if count >= 0 { json!(count) } else { Value::Null },
            "writable": t.to_ascii_lowercase().starts_with("wb_"),
        }));
    }
    Json(json!({ "tables": tables, "path": db_path().to_string_lossy() })).into_response()
}

#[derive(serde::Deserialize)]
pub struct RowsParams {
    table: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    sort: Option<String>,
    dir: Option<String>,
}

async fn rows(State(state): State<AppState>, Query(p): Query<RowsParams>) -> Response {
    let Some(table) = p.table.filter(|t| !t.is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "table required" })),
        )
            .into_response();
    };
    let conn = match state.store.read() {
        Ok(c) => c,
        Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, e.to_string()).into_response(),
    };
    // The table name cannot be bound as a parameter, so it is verified against
    // sqlite_master rather than escaped-and-hoped. An identifier that is not a
    // real table never reaches a query string.
    let known: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name=?1",
            [&table],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !known {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("no such table: {table}") })),
        )
            .into_response();
    }
    let limit = p.limit.unwrap_or(100).min(1000);
    let offset = p.offset.unwrap_or(0);
    let columns: Vec<String> = conn
        .prepare(&format!(
            "PRAGMA table_info(\"{}\")",
            table.replace('"', "\"\"")
        ))
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default();
    let sort = p
        .sort
        .filter(|column| columns.iter().any(|known| known == column));
    let direction = if p.dir.as_deref() == Some("desc") {
        "DESC"
    } else {
        "ASC"
    };
    let order = sort
        .as_deref()
        .map(|column| format!(" ORDER BY \"{}\" {direction}", column.replace('"', "\"\"")))
        .unwrap_or_default();
    let sql = format!(
        "SELECT * FROM \"{}\"{} LIMIT {} OFFSET {}",
        table.replace('"', "\"\""),
        order,
        limit,
        offset
    );
    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{}\"", table.replace('"', "\"\"")),
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    match conn
        .prepare(&sql)
        .and_then(|mut s| rows_to_json(&mut s, limit))
    {
        Ok((columns, rows, _)) => Json(json!({
            "columns": columns,
            "rows": rows,
            "table": table,
            "total": total,
            "limit": limit,
            "offset": offset,
            "sort": sort,
            "dir": direction.to_ascii_lowercase(),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Text-to-SQL: takes a natural-language question, builds a prompt with the
/// DB schema, calls the helper model to generate a SELECT query, runs it
/// read-only, and returns both the SQL and the result set. Supports
/// conversational refinement via an optional `history` array.
async fn ask_sql(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let question = body
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if question.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "question required" })),
        )
            .into_response();
    }

    let started = Instant::now();

    // Build a compact schema summary for the prompt.
    let schema_text = {
        let conn = match state.store.read() {
            Ok(c) => c,
            Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, e.to_string()).into_response(),
        };
        let names: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .and_then(|mut s| {
                s.query_map([], |r| r.get::<_, String>(0))
                    .map(|r| r.flatten().collect())
            })
            .unwrap_or_default();
        let mut buf = String::new();
        for t in &names {
            let cols: Vec<(String, String)> = conn
                .prepare(&format!(
                    "PRAGMA table_info(\"{}\")",
                    t.replace('"', "\"\"")
                ))
                .and_then(|mut s| {
                    s.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
                        .map(|r| r.flatten().collect())
                })
                .unwrap_or_default();
            let col_list: String = cols
                .iter()
                .map(|(name, typ)| format!("{name} {typ}"))
                .collect::<Vec<_>>()
                .join(", ");
            buf.push_str(&format!("{t}({col_list})\n"));
        }
        buf
    };

    // Assemble the prompt with schema context and optional conversation history.
    let history = body
        .get("history")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut prompt = format!(
        "You are a SQLite expert. Given the database schema below, write a single SELECT query that answers the user's question.\n\
         \n\
         Rules:\n\
         - Output ONLY the SQL query, nothing else. No explanation, no markdown fences, no comments.\n\
         - The query must be read-only (SELECT, WITH, or EXPLAIN only).\n\
         - Use appropriate JOINs, aggregations, and WHERE clauses.\n\
         - Limit results to 200 rows unless the user asks for a specific count.\n\
         - Use double-quotes for identifiers that need quoting.\n\
         - For text searches, prefer LIKE with % wildcards (case-insensitive).\n\
         \n\
         Schema:\n\
         {schema_text}\n"
    );

    // Append conversation history for iterative refinement.
    for turn in &history {
        if let (Some(q), Some(s)) = (
            turn.get("question").and_then(Value::as_str),
            turn.get("sql").and_then(Value::as_str),
        ) {
            prompt.push_str(&format!("User asked: {q}\nYou wrote: {s}\n"));
            if let Some(err) = turn.get("error").and_then(Value::as_str) {
                prompt.push_str(&format!("That query failed with: {err}\n"));
            } else if let Some(rc) = turn.get("rowcount").and_then(Value::as_u64) {
                prompt.push_str(&format!("It returned {rc} rows.\n"));
            }
        }
    }
    prompt.push_str(&format!("User question: {question}\n"));

    // Call the helper model.
    let (via, raw_answer) = match super::lookup::helper_answer(&prompt).await {
        Ok(pair) => pair,
        Err((status, msg)) => {
            return (status, Json(json!({ "error": msg, "measured": false }))).into_response();
        }
    };

    let model_ms = started.elapsed().as_millis();

    // Extract SQL from the response: strip markdown fences if present, take
    // the first statement.
    let sql = extract_sql(&raw_answer);
    if sql.is_empty() {
        return Json(json!({
            "error": "The model did not produce a valid SQL query.",
            "raw": raw_answer,
            "via": via,
            "ms": model_ms,
            "measured": true,
        }))
        .into_response();
    }

    // Verify it is read-only before running.
    if sql_is_write(&sql) {
        return Json(json!({
            "sql": sql,
            "error": "The generated query is not read-only. Only SELECT queries are allowed.",
            "via": via,
            "ms": model_ms,
            "measured": true,
        }))
        .into_response();
    }

    // Run read-only.
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
        | rusqlite::OpenFlags::SQLITE_OPEN_URI;
    let conn = match rusqlite::Connection::open_with_flags(db_path(), flags) {
        Ok(c) => c,
        Err(e) => {
            return Json(json!({
                "sql": sql,
                "error": e.to_string(),
                "via": via,
                "ms": model_ms,
                "measured": true,
            }))
            .into_response();
        }
    };
    match conn
        .prepare(&sql)
        .and_then(|mut s| rows_to_json(&mut s, SQL_ROW_CAP))
    {
        Ok((columns, rows, truncated)) => {
            let rowcount = rows.len();
            let total_ms = started.elapsed().as_millis();
            Json(json!({
                "sql": sql,
                "columns": columns,
                "rows": rows,
                "rowcount": rowcount,
                "truncated": truncated,
                "via": via,
                "model_ms": model_ms,
                "query_ms": total_ms - model_ms,
                "ms": total_ms,
                "measured": true,
            }))
            .into_response()
        }
        Err(e) => Json(json!({
            "sql": sql,
            "error": e.to_string(),
            "via": via,
            "model_ms": model_ms,
            "ms": started.elapsed().as_millis(),
            "measured": true,
        }))
        .into_response(),
    }
}

/// Strip markdown fences and leading/trailing whitespace from model output.
fn extract_sql(raw: &str) -> String {
    let trimmed = raw.trim();
    // Strip ```sql ... ``` or ``` ... ```
    let body = if trimmed.starts_with("```") {
        let after_fence = trimmed
            .strip_prefix("```sql")
            .or_else(|| trimmed.strip_prefix("```SQL"))
            .or_else(|| trimmed.strip_prefix("```sqlite"))
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        after_fence
            .strip_suffix("```")
            .unwrap_or(after_fence)
            .trim()
    } else {
        trimmed
    };
    // Take the first complete statement (up to the first semicolon, inclusive).
    if let Some(pos) = body.find(';') {
        body[..=pos].trim().to_string()
    } else {
        body.trim().to_string()
    }
}

async fn run_sql(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let sql = body
        .get("sql")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if sql.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "sql required" })),
        )
            .into_response();
    }
    let allow_write = body.get("write").and_then(Value::as_bool).unwrap_or(false);
    let write = sql_is_write(&sql);
    let started = Instant::now();

    if !write {
        // READ-ONLY BY CONSTRUCTION. Not a keyword check — the engine refuses.
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI;
        let conn = match rusqlite::Connection::open_with_flags(db_path(), flags) {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        };
        return match conn
            .prepare(&sql)
            .and_then(|mut s| rows_to_json(&mut s, SQL_ROW_CAP))
        {
            Ok((columns, rows, truncated)) => {
                let rowcount = rows.len();
                Json(json!({
                    "columns": columns,
                    "rows": rows,
                    "rowcount": rowcount,
                    "truncated": truncated,
                    "ms": started.elapsed().as_millis(),
                    "readonly": true,
                }))
                .into_response()
            }
            // SQLITE_READONLY surfaces here with its own message; pass it
            // through rather than rewriting it, so a refused write says it was
            // refused instead of looking like a syntax error.
            Err(e) => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response(),
        };
    }

    if !allow_write {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Read-only mode — enable ‘Allow writes’ to run this statement."
            })),
        )
            .into_response();
    }
    if let Some(error) = sql_write_guard(&sql) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }

    // write: true — the user asked for it explicitly via the UI toggle. Goes
    // through the store's writer so it serialises with every other mutation
    // rather than racing them on a second connection.
    let sql2 = sql.clone();
    let changed = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let cw = changed.clone();
    let res = state
        .store
        .write_async(move |conn| {
            let n = conn.execute_batch(&sql2).map(|_| conn.changes() as usize)?;
            *cw.lock().expect("slot") = n;
            Ok(crate::db::WriteOutcome {
                applied: true,
                events: vec![],
            })
        })
        .await;
    match res {
        Ok(_) => {
            let n = *changed.lock().expect("slot");
            Json(json!({
                "write": true,
                "rowcount": n,
                "changed": n,
                "ms": started.elapsed().as_millis(),
                "message": format!("OK — {n} row(s) affected"),
                "readonly": false,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

fn sql_is_write(sql: &str) -> bool {
    let without_comments = regex::Regex::new(r"(?s)--[^\n]*|/\*.*?\*/")
        .expect("sql comment regex")
        .replace_all(sql, "");
    let first = without_comments
        .trim_start()
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    !matches!(
        first.as_str(),
        "select" | "with" | "pragma" | "explain" | "values"
    )
}

/// The workbench is a sandbox, not an alternate mutation API for Amux's own
/// state.  Writes are opt-in and every identified target must be `wb_*`.
fn sql_write_guard(sql: &str) -> Option<String> {
    let target_re = regex::Regex::new(
        r#"(?ix)\b(?:
            insert\s+(?:or\s+\w+\s+)?into |
            replace\s+into |
            update |
            delete\s+from |
            create\s+(?:temp\s+|temporary\s+)?table(?:\s+if\s+not\s+exists)? |
            drop\s+table(?:\s+if\s+exists)? |
            alter\s+table |
            create\s+index(?:\s+if\s+not\s+exists)?\s+\w+\s+on
        )\s+["'`]?([A-Za-z_][A-Za-z0-9_]*)"#,
    )
    .expect("sql write target regex");
    let targets: Vec<&str> = target_re
        .captures_iter(sql)
        .filter_map(|capture| capture.get(1).map(|found| found.as_str()))
        .collect();
    let mut bad: Vec<&str> = targets
        .iter()
        .copied()
        .filter(|target| !target.to_ascii_lowercase().starts_with("wb_"))
        .collect();
    bad.sort_unstable();
    bad.dedup();
    if !bad.is_empty() {
        return Some(format!(
            "Write blocked: only tables named wb_* are writable (protects Amux state). Offending: {}",
            bad.join(", ")
        ));
    }
    if targets.is_empty() {
        return Some(
            "Write blocked: could not identify a wb_* target table. Workbench writes must target tables named wb_*."
                .to_string(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{extract_sql, sql_is_write, sql_write_guard};

    #[test]
    fn extract_sql_strips_fences_and_takes_first_statement() {
        assert_eq!(extract_sql("```sql\nSELECT 1;\n```"), "SELECT 1;");
        assert_eq!(extract_sql("```\nSELECT 1;\n```"), "SELECT 1;");
        assert_eq!(
            extract_sql("  SELECT * FROM t LIMIT 10  "),
            "SELECT * FROM t LIMIT 10"
        );
        assert_eq!(extract_sql("SELECT 1; DROP TABLE x;"), "SELECT 1;");
    }

    #[test]
    fn query_mode_distinguishes_reads_from_writes_after_comments() {
        assert!(!sql_is_write("-- inspect\nSELECT * FROM issues"));
        assert!(!sql_is_write(
            "/* explain */ WITH x AS (SELECT 1) SELECT * FROM x"
        ));
        assert!(sql_is_write("UPDATE wb_notes SET value='x'"));
    }

    #[test]
    fn writes_are_confined_to_workbench_tables() {
        assert!(sql_write_guard("CREATE TABLE wb_notes(id INTEGER)").is_none());
        assert!(sql_write_guard("INSERT INTO wb_notes VALUES (1)").is_none());
        let blocked = sql_write_guard("UPDATE issues SET status='done'").unwrap();
        assert!(blocked.contains("issues"), "{blocked}");
        assert!(sql_write_guard("VACUUM")
            .unwrap()
            .contains("could not identify"));
    }
}

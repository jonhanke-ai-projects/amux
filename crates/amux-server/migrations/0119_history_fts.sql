-- Full-text index over every message in `cmd_history` (the MSG-<id> rows).
--
-- `/api/history?q=` was `text LIKE '%q%'`: one exact substring, unranked, a
-- scan of the table. Measured 2026-10-13 on the owner's DB (19,176 rows):
-- `q=reaper` found rows while `q=browser routing` found NONE, because the two
-- words never sat side by side in that order. People type words, not phrases.
--
-- WHY A SEPARATE INDEX AND NOT `search_docs`. AF-499 deliberately put only
-- type='user' rows into universal search, because the rest is machine traffic
-- and would drown the curated corpus. This index is for SEARCHING THE MESSAGE
-- LOG ITSELF, where machine traffic is exactly what is being searched for, so it
-- covers every row and lives beside `search_docs` rather than inside it.
--
-- External content (the text is stored once, in cmd_history) kept current by
-- triggers, the same mechanism every family in 0013_search.sql uses, so no
-- writer can skip it. `text` is effectively immutable, but an UPDATE OF text
-- trigger keeps the index honest if it ever changes.

CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(
    text,
    content='cmd_history',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS history_fts_ai AFTER INSERT ON cmd_history BEGIN
    INSERT INTO history_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS history_fts_ad AFTER DELETE ON cmd_history BEGIN
    INSERT INTO history_fts(history_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER IF NOT EXISTS history_fts_au AFTER UPDATE OF text ON cmd_history BEGIN
    INSERT INTO history_fts(history_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO history_fts(rowid, text) VALUES (new.id, new.text);
END;

-- Backfill: rebuild reads every row of the content table.
INSERT INTO history_fts(history_fts) VALUES ('rebuild');

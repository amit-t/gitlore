-- gitlore index — schema migration 0004 (TDD-001 §2.2, FTS5 content fix).
--
-- The original commits_fts table was declared with `content=''` (contentless
-- FTS5). In contentless mode SQLite does not persist column values in the FTS5
-- shadow tables, so `SELECT sha FROM commits_fts` returns empty strings even
-- after an explicit INSERT.  The lexical search JOIN on `commits_fts.sha =
-- commits.sha` therefore never matched any row, producing zero search results.
--
-- This migration drops commits_fts and re-creates it as a regular (content-
-- bearing) FTS5 table.  All data is considered stale; the fts5_populated flag
-- is reset to 'false' so the indexer's backfill code re-inserts from commits.

DROP TABLE IF EXISTS commits_fts;

CREATE VIRTUAL TABLE commits_fts USING fts5(
    sha UNINDEXED,
    subject,
    body,
    expanded,
    paths,
    tokenize='unicode61 remove_diacritics 2'
);

-- Reset backfill marker so the indexer repopulates on next open.
UPDATE index_state SET value = 'false' WHERE key = 'fts5_populated';

UPDATE index_state SET value = '4' WHERE key = 'schema_version';

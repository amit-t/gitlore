-- gitlore index — schema migration 0003 (TDD-001 §2.2, grill #24).
--
-- Adds a backfill marker for FTS5 population. When the commits table is
-- empty (fresh index), fts5_populated is set to 'true' because there is
-- nothing to backfill. When commits exist, it is set to 'false' to signal
-- that a backfill of commits_fts is required.

INSERT INTO index_state(key, value)
SELECT 'fts5_populated',
       CASE WHEN (SELECT COUNT(*) FROM commits) = 0 THEN 'true' ELSE 'false' END
WHERE NOT EXISTS (SELECT 1 FROM index_state WHERE key = 'fts5_populated');

UPDATE index_state SET value = '3' WHERE key = 'schema_version';

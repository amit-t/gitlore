-- gitlore index — schema migration 0002 (M3-4 identity resolution).
--
-- Adds a per-identity bot flag so the override + heuristic resolvers in
-- `index::identity` can persist their classification verdict on disk and
-- downstream consumers (story clusterer, contributor stats) can filter
-- bots without re-running the heuristic on every read.
--
-- Default `0` keeps the column populated for every pre-existing row from
-- migration 0001 without a separate backfill pass.

ALTER TABLE identities ADD COLUMN is_bot INTEGER NOT NULL DEFAULT 0;

UPDATE index_state SET value = '2' WHERE key = 'schema_version';

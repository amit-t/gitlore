//! Versioned migration runner for the gitlore index (TDD-000 §2.2).
//!
//! The runner is a single function, [`migrate`], that brings a freshly
//! opened SQLite connection up to [`LATEST`]:
//!
//! 1. Detects whether `index_state` exists. On a fresh database it does
//!    not; `migrate` runs the canonical migration `0001_init.sql`
//!    unconditionally and treats the resulting `schema_version` as the
//!    current version.
//! 2. Otherwise reads `index_state.schema_version` and validates it
//!    against [`LATEST`]:
//!    * `current_version > LATEST` → [`Error::SchemaVersionTooNew`] so the
//!      caller can prompt for a binary upgrade rather than silently
//!      mangling a newer-on-disk schema (SPEC-001 §4.3).
//!    * `current_version == LATEST` → no-op; returns `LATEST`.
//!    * `current_version < LATEST` → applies every migration in numbered
//!      order. Each migration runs inside its own `BEGIN`/`COMMIT` so a
//!      mid-migration crash leaves the database at the previous version
//!      rather than half-converted.
//!
//! ## Adding a migration
//!
//! Drop the new SQL file alongside `0001_init.sql`, then append a
//! `(version, sql)` entry to [`MIGRATIONS`] using `include_str!`. Bump
//! [`LATEST`] and add an `UPDATE index_state SET value = '<N>' WHERE
//! key = 'schema_version';` at the bottom of the new file. The migration
//! runner enforces strictly ascending versions.

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Highest schema version this binary understands. Bump alongside every
/// new migration file.
pub const LATEST: u32 = 3;

/// All known migrations, in ascending version order.
///
/// Each entry is `(version, sql)` where `sql` is the entire body of the
/// matching `NNNN_*.sql` file (`include_str!`-embedded so the migrations
/// travel with the binary).
const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("0001_init.sql")),
    (2, include_str!("0002_identity_is_bot.sql")),
    (3, include_str!("0003_fts5_backfill_marker.sql")),
];

/// Bring `conn` up to [`LATEST`].
///
/// Returns the schema version observable on disk after the call. On a
/// fresh database this is [`LATEST`]; on an already-current database the
/// function is a no-op that still returns [`LATEST`].
///
/// # Errors
///
/// * [`Error::SchemaVersionTooNew`] when the on-disk version is higher
///   than this binary supports.
/// * [`Error::Sqlite`] for any underlying SQLite failure (statement
///   compilation, transaction rollback, etc.).
pub fn migrate(conn: &mut Connection) -> Result<u32> {
    // Detect whether the database has been initialised. `index_state` is
    // the canonical marker: every migration ends with a write to its
    // `schema_version` row, so its presence implies the database has been
    // through at least migration 0001.
    let has_state_table = sqlite_master_has(conn, "index_state").map_err(sqlite_err)?;

    let current = if has_state_table {
        read_schema_version(conn)?
    } else {
        0
    };

    if current > LATEST {
        return Err(Error::SchemaVersionTooNew {
            found: current,
            supported: LATEST,
        });
    }

    // Migrations whose number is strictly greater than `current` are the
    // ones we still need to apply. The set is statically sorted by
    // construction, so iterating in order is safe.
    for &(version, sql) in MIGRATIONS {
        if version <= current {
            continue;
        }
        apply_one(conn, version, sql)?;
    }

    // Re-read so the returned value reflects what the last migration
    // wrote, not a value we computed in-memory. Cheap (one indexed
    // lookup) and catches a migration that forgot to write its row.
    read_schema_version(conn)
}

/// Apply a single migration inside its own transaction.
fn apply_one(conn: &mut Connection, version: u32, sql: &str) -> Result<()> {
    let tx = conn.transaction().map_err(sqlite_err)?;
    tx.execute_batch(sql).map_err(sqlite_err)?;
    tx.commit().map_err(sqlite_err)?;
    tracing::debug!(version, "applied gitlore index migration");
    Ok(())
}

/// Return `true` iff `sqlite_master` lists a table named `name`.
fn sqlite_master_has(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    let mut stmt =
        conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    stmt.exists([name])
}

/// Read `index_state.schema_version` and parse it as `u32`. A missing row
/// or a non-numeric value reads as `0`; SQLite errors propagate as
/// [`Error::Sqlite`].
fn read_schema_version(conn: &Connection) -> Result<u32> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM index_state WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(String::new()),
            other => Err(other),
        })
        .map(Some)
        .map_err(sqlite_err)?;

    Ok(raw
        .as_deref()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0))
}

/// Convert a `rusqlite::Error` into the workspace [`Error::Sqlite`]
/// variant, preserving the original Display payload.
fn sqlite_err(e: rusqlite::Error) -> Error {
    Error::Sqlite(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_in_memory() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn migrate_fresh_db_creates_all_tables() {
        let mut conn = open_in_memory();
        let v = migrate(&mut conn).unwrap();
        assert_eq!(v, LATEST);

        for table in [
            "commits",
            "identities",
            "identity_aliases",
            "commit_coauthors",
            "commit_refs",
            "tags",
            "commits_fts",
            "stories",
            "story_members",
            "path_stats",
            "repo_stats",
            "index_state",
        ] {
            assert!(
                sqlite_master_has_any(&conn, table),
                "missing table {table} after migrate()"
            );
        }
    }

    #[test]
    fn migrate_writes_schema_version_row() {
        let mut conn = open_in_memory();
        migrate(&mut conn).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM index_state WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v, LATEST.to_string());
    }

    #[test]
    fn migrate_does_not_omit_classification_counters() {
        let mut conn = open_in_memory();
        migrate(&mut conn).unwrap();
        let columns = pragma_columns(&conn, "commits");
        for c in [
            "test_files_changed",
            "config_files_changed",
            "infra_files_changed",
            "doc_files_changed",
            "code_files_changed",
            "dependency_files_changed",
            "ci_files_changed",
            "fixture_files_changed",
            "migration_files_changed",
        ] {
            assert!(columns.contains(&c.to_string()), "commits missing {c}");
        }
    }

    #[test]
    fn migrate_does_not_create_commit_vectors() {
        // commit_vectors lands at M11 via setup-embeddings, not M3-2.
        let mut conn = open_in_memory();
        migrate(&mut conn).unwrap();
        assert!(
            !sqlite_master_has_any(&conn, "commit_vectors"),
            "commit_vectors must not be created by migration 0001"
        );
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut conn = open_in_memory();
        migrate(&mut conn).unwrap();
        // Insert a row so a re-run that mistakenly re-applied 0001 would
        // wipe or duplicate it.
        conn.execute(
            "INSERT INTO commits (sha, author_name, author_email, committer_name, \
             committer_email, authored_at, committed_at, subject, indexed_at, updated_at) \
             VALUES ('a', 'A', 'a@x', 'A', 'a@x', 1, 1, 'fix', 1, 1)",
            [],
        )
        .unwrap();
        let v = migrate(&mut conn).unwrap();
        assert_eq!(v, LATEST);
        let count: u32 = conn
            .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "idempotent migrate dropped data");
    }

    #[test]
    fn migrate_rejects_too_new_schema() {
        let mut conn = open_in_memory();
        migrate(&mut conn).unwrap();
        conn.execute(
            "UPDATE index_state SET value = '999' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        let err = migrate(&mut conn).unwrap_err();
        assert_eq!(err.code(), "schema_version_too_new");
        match err {
            Error::SchemaVersionTooNew { found, supported } => {
                assert_eq!(found, 999);
                assert_eq!(supported, LATEST);
            }
            other => panic!("expected SchemaVersionTooNew, got {other:?}"),
        }
    }

    #[test]
    fn migrate_returns_latest_for_already_current_db() {
        let mut conn = open_in_memory();
        migrate(&mut conn).unwrap();
        let v = migrate(&mut conn).unwrap();
        assert_eq!(v, LATEST);
    }

    #[test]
    fn migrate_0003_sets_fts5_populated_true_when_commits_empty() {
        let mut conn = open_in_memory();
        // Run migrations 0001 and 0002 only
        let tx = conn.transaction().unwrap();
        tx.execute_batch(include_str!("0001_init.sql")).unwrap();
        tx.execute_batch(include_str!("0002_identity_is_bot.sql"))
            .unwrap();
        tx.commit().unwrap();

        // Apply migration 0003 on empty commits table
        let tx = conn.transaction().unwrap();
        tx.execute_batch(include_str!("0003_fts5_backfill_marker.sql"))
            .unwrap();
        tx.commit().unwrap();

        // Verify fts5_populated is 'true' when commits is empty
        let fts5_populated: String = conn
            .query_row(
                "SELECT value FROM index_state WHERE key = 'fts5_populated'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts5_populated, "true");

        // Verify schema_version is 3
        let schema_version: String = conn
            .query_row(
                "SELECT value FROM index_state WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, "3");
    }

    #[test]
    fn migrate_0003_sets_fts5_populated_false_when_commits_exist() {
        let mut conn = open_in_memory();
        // Run migrations 0001 and 0002 only
        let tx = conn.transaction().unwrap();
        tx.execute_batch(include_str!("0001_init.sql")).unwrap();
        tx.execute_batch(include_str!("0002_identity_is_bot.sql"))
            .unwrap();
        tx.commit().unwrap();

        // Insert a commit before migration 0003
        conn.execute(
            "INSERT INTO commits (sha, author_name, author_email, committer_name, \
             committer_email, authored_at, committed_at, subject, indexed_at, updated_at) \
             VALUES ('abc123', 'Test', 'test@example.com', 'Test', 'test@example.com', \
             1234567890, 1234567890, 'Test commit', 1234567890, 1234567890)",
            [],
        )
        .unwrap();

        // Apply migration 0003 with existing commits
        let tx = conn.transaction().unwrap();
        tx.execute_batch(include_str!("0003_fts5_backfill_marker.sql"))
            .unwrap();
        tx.commit().unwrap();

        // Verify fts5_populated is 'false' when commits exist
        let fts5_populated: String = conn
            .query_row(
                "SELECT value FROM index_state WHERE key = 'fts5_populated'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts5_populated, "false");

        // Verify schema_version is 3
        let schema_version: String = conn
            .query_row(
                "SELECT value FROM index_state WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, "3");
    }

    fn sqlite_master_has_any(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE name = ?1",
            [name],
            |_| Ok(()),
        )
        .is_ok()
    }

    fn pragma_columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        let cols = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        cols
    }
}

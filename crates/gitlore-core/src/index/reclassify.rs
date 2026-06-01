//! Bulk re-classification of indexed commits (M3-5, TDD-000 §2.2).
//!
//! When the user changes their `[classification]` config, gitlore needs
//! to recompute the nine per-commit `*_files_changed` counters without
//! re-walking history. [`reclassify_all`] reads each commit's
//! `files_changed` JSON, re-runs the [`Classifier`] over each entry,
//! and writes the updated counters in a single transaction so a
//! partial run does not leave the index half-updated.
//!
//! `is_revert` is intentionally untouched in M3-5 — the revert detector
//! lands at M3-6.
//!
//! ## Mapping `Category` → SQL counters
//!
//! The Q14 [`Category`] enum has nine variants but the SPEC-001 §5.1
//! schema declares a slightly different set of nine counters
//! (`dependency_files_changed`, `fixture_files_changed` instead of
//! generated/asset categories). The mapping in [`bump_counter`] handles
//! that asymmetry: matched categories increment the obvious column,
//! and the asymmetric pair (`Generated`, `Asset`) currently have no
//! column to bump — those counts are simply not persisted in M3-5.

use rusqlite::{params, Connection};

use crate::error::{Error, Result};
use crate::index::classify::{Category, Classifier};
use crate::index::schema::parse_file_changes;

/// Progress callback fired every [`PROGRESS_INTERVAL`] rows during
/// [`reclassify_all`].
///
/// The argument is the running count of rows the transaction has
/// updated so far. Callbacks must be cheap — they run on the same
/// thread that holds the writer lock.
pub type ProgressFn<'a> = dyn FnMut(u64) + 'a;

/// Number of rows between progress callbacks.
pub const PROGRESS_INTERVAL: u64 = 1000;

/// Per-category counter aggregation for a single commit's
/// `files_changed` list.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counters {
    test: u32,
    config: u32,
    infra: u32,
    doc: u32,
    code: u32,
    ci: u32,
    migration: u32,
    // dependency_files_changed and fixture_files_changed are part of
    // the schema but have no current Category mapping (see module
    // docs). Tracked as Option-shaped no-ops via direct column writes
    // below.
}

impl Counters {
    fn bump(&mut self, cat: Category) {
        match cat {
            Category::Test => self.test += 1,
            Category::Config => self.config += 1,
            Category::Infra => self.infra += 1,
            Category::Docs => self.doc += 1,
            Category::Code => self.code += 1,
            Category::Ci => self.ci += 1,
            Category::Migration => self.migration += 1,
            // No counter column in the current schema (M3-5):
            Category::Generated | Category::Asset => {}
        }
    }
}

/// Re-classify every row in `commits` using `classifier`, updating the
/// per-commit counters in a single transaction.
///
/// Returns the number of rows that were updated. Progress is reported
/// every [`PROGRESS_INTERVAL`] rows via the supplied callback (see
/// [`reclassify_all_with_progress`] for a tighter caller surface).
///
/// # Errors
///
/// * [`Error::Sqlite`] for any SQLite error during the scan or the
///   batched update.
pub fn reclassify_all(conn: &mut Connection, classifier: &Classifier) -> Result<u64> {
    reclassify_all_with_progress(conn, classifier, &mut |_| {})
}

/// As [`reclassify_all`], but with an explicit progress callback. The
/// callback receives the running row count every [`PROGRESS_INTERVAL`]
/// rows.
pub fn reclassify_all_with_progress(
    conn: &mut Connection,
    classifier: &Classifier,
    progress: &mut ProgressFn<'_>,
) -> Result<u64> {
    // Snapshot the (sha, files_changed) pairs first so we can release
    // the SELECT statement before opening the transaction. This keeps
    // `conn` available for the `.transaction()` borrow below and
    // avoids holding two prepared statements concurrently.
    let rows: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT sha, files_changed FROM commits")
            .map_err(sqlite_err)?;
        let mapped = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_err)?;
        let mut collected = Vec::new();
        for r in mapped {
            collected.push(r.map_err(sqlite_err)?);
        }
        collected
    };

    let tx = conn.transaction().map_err(sqlite_err)?;
    let mut updated: u64 = 0;
    {
        let mut update = tx
            .prepare(
                "UPDATE commits SET \
                   test_files_changed = ?2, \
                   config_files_changed = ?3, \
                   infra_files_changed = ?4, \
                   doc_files_changed = ?5, \
                   code_files_changed = ?6, \
                   ci_files_changed = ?7, \
                   migration_files_changed = ?8 \
                 WHERE sha = ?1",
            )
            .map_err(sqlite_err)?;
        for (sha, files_changed) in &rows {
            let counters = classify_files(classifier, files_changed);
            let n = update
                .execute(params![
                    sha,
                    counters.test,
                    counters.config,
                    counters.infra,
                    counters.doc,
                    counters.code,
                    counters.ci,
                    counters.migration,
                ])
                .map_err(sqlite_err)?;
            updated += n as u64;
            if updated.is_multiple_of(PROGRESS_INTERVAL) {
                progress(updated);
            }
        }
    }
    tx.commit().map_err(sqlite_err)?;
    Ok(updated)
}

/// Walk a commit's `files_changed` JSON and aggregate per-category
/// counts. Malformed JSON yields zero counters (matches the
/// best-effort decoding contract in [`parse_file_changes`]).
fn classify_files(classifier: &Classifier, files_changed: &str) -> Counters {
    let mut counters = Counters::default();
    for change in parse_file_changes(files_changed) {
        counters.bump(classifier.classify(&change.path));
    }
    counters
}

fn sqlite_err(e: rusqlite::Error) -> Error {
    Error::Sqlite(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::migrations::migrate;
    use crate::index::schema::{serialize_file_changes, FileChangeRecord};
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn open_initialised_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        conn
    }

    fn insert_commit(conn: &Connection, sha: &str, files: &[FileChangeRecord]) {
        let files_changed = serialize_file_changes(files);
        conn.execute(
            "INSERT INTO commits (sha, author_name, author_email, committer_name, \
             committer_email, authored_at, committed_at, subject, files_changed, \
             indexed_at, updated_at) \
             VALUES (?1, 'A', 'a@x', 'A', 'a@x', 1, 1, 'fix', ?2, 1, 1)",
            params![sha, files_changed],
        )
        .unwrap();
    }

    fn fake_change(path: &str) -> FileChangeRecord {
        FileChangeRecord {
            path: path.into(),
            status: 'M',
            insertions: 1,
            deletions: 0,
        }
    }

    #[test]
    fn reclassify_updates_counters_from_files_changed() {
        let mut conn = open_initialised_db();
        let dir = tempdir().unwrap();
        // Drop Cargo.toml so we get the rust ecosystem code globs.
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        let classifier = Classifier::default_for(dir.path()).unwrap();

        insert_commit(
            &conn,
            "aaa",
            &[
                fake_change("src/lib.rs"),
                fake_change("tests/it.rs"),
                fake_change("README.md"),
                fake_change("Cargo.toml"),
            ],
        );

        let n = reclassify_all(&mut conn, &classifier).unwrap();
        assert_eq!(n, 1);

        let (test_n, config_n, doc_n, code_n): (u32, u32, u32, u32) = conn
            .query_row(
                "SELECT test_files_changed, config_files_changed, doc_files_changed, \
                 code_files_changed FROM commits WHERE sha = 'aaa'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(test_n, 1, "tests/it.rs should be Test");
        assert_eq!(config_n, 1, "Cargo.toml should be Config");
        assert_eq!(doc_n, 1, "README.md should be Docs");
        assert_eq!(code_n, 1, "src/lib.rs should be Code");
    }

    #[test]
    fn reclassify_leaves_is_revert_unchanged() {
        let mut conn = open_initialised_db();
        let dir = tempdir().unwrap();
        let classifier = Classifier::default_for(dir.path()).unwrap();

        insert_commit(&conn, "rev", &[fake_change("src/lib.rs")]);
        conn.execute("UPDATE commits SET is_revert = 1 WHERE sha = 'rev'", [])
            .unwrap();

        reclassify_all(&mut conn, &classifier).unwrap();

        let is_revert: u8 = conn
            .query_row("SELECT is_revert FROM commits WHERE sha = 'rev'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(is_revert, 1, "M3-5 must not touch is_revert");
    }

    #[test]
    fn reclassify_handles_empty_commits_table() {
        let mut conn = open_initialised_db();
        let dir = tempdir().unwrap();
        let classifier = Classifier::default_for(dir.path()).unwrap();
        let n = reclassify_all(&mut conn, &classifier).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn reclassify_progress_callback_fires_every_thousand_rows() {
        let mut conn = open_initialised_db();
        let dir = tempdir().unwrap();
        let classifier = Classifier::default_for(dir.path()).unwrap();

        for i in 0..(PROGRESS_INTERVAL as usize * 2) {
            insert_commit(&conn, &format!("{i:040x}"), &[fake_change("src/lib.rs")]);
        }

        let mut ticks: Vec<u64> = Vec::new();
        let n =
            reclassify_all_with_progress(&mut conn, &classifier, &mut |c| ticks.push(c)).unwrap();
        assert_eq!(n, PROGRESS_INTERVAL * 2);
        assert_eq!(ticks, vec![PROGRESS_INTERVAL, PROGRESS_INTERVAL * 2]);
    }
}

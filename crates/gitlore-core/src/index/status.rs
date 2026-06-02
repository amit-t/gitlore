//! Read-only index status reader (M3-7, SPEC-001 §4.1 / §4.3.1).
//!
//! Opens the SQLite index with `SQLITE_OPEN_READ_ONLY` so the writer lock
//! is never contended and the read path itself cannot mutate the
//! database. Used by `gitlore status` to render the index header
//! (commit count, schema version, embeddings state, db size, holder of
//! the writer lock if any).
//!
//! Returning a closed struct (no `Connection` leak) keeps `rusqlite`
//! encapsulated inside `gitlore-core` so the binary crate does not need
//! to depend on it directly.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::git::GitProvider;
use crate::index::indexer::{INDEX_DB_FILENAME, INDEX_LOCK_FILENAME};
use crate::index::storage::resolve_index_path;

/// Snapshot of index metadata returned by [`StatusReport::read`].
///
/// `writer_lock` is populated when the lockfile exists *and* parses
/// cleanly; it is `None` when the lockfile is missing, empty, or
/// truncated mid-write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    /// Absolute path to the SQLite database file.
    pub db_path: PathBuf,
    /// `commits` row count.
    pub commit_count: u64,
    /// On-disk size of the database file in bytes (does not include the
    /// WAL or SHM siblings).
    pub db_size_bytes: u64,
    /// Schema version stamped by the migration runner. `0` when the
    /// database exists but has no `index_state` row yet.
    pub schema_version: u32,
    /// `true` iff `commit_vectors` exists and the `embeddings_enabled`
    /// state row is set to `1`.
    pub embeddings_enabled: bool,
    /// Embedding model name, when one is recorded in `index_state`
    /// (`embeddings.model_name`).
    pub model: Option<String>,
    /// Writer lock holder identity (PID + RFC-3339 acquire time), if
    /// the lockfile is present and parseable.
    pub writer_lock: Option<WriterLockInfo>,
}

/// PID + RFC-3339 acquire timestamp recorded by the writer lock holder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriterLockInfo {
    /// Holder PID as recorded in the lockfile payload.
    pub pid: u32,
    /// RFC-3339 timestamp the lock was acquired at.
    pub started_at: String,
}

impl StatusReport {
    /// Read the index status for the repository rooted at `repo_root`,
    /// using `provider` to resolve the Git common dir.
    ///
    /// Opens the SQLite connection with `SQLITE_OPEN_READ_ONLY` so no
    /// writer-lock contention occurs and the operation is safe to run
    /// alongside a long-running `gitlore index`.
    pub fn read(repo_root: &Path, provider: &dyn GitProvider) -> Result<Self> {
        let location = resolve_index_path(repo_root, provider)?;
        let dir = location.path().to_path_buf();
        let db_path = dir.join(INDEX_DB_FILENAME);
        let lock_path = dir.join(INDEX_LOCK_FILENAME);

        let db_size_bytes = fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
        let writer_lock = read_lock_payload(&lock_path);

        if !db_path.exists() {
            return Ok(Self {
                db_path,
                commit_count: 0,
                db_size_bytes,
                schema_version: 0,
                embeddings_enabled: false,
                model: None,
                writer_lock,
            });
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&db_path, flags)
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        let commit_count: u64 = conn
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        let schema_version: u32 = conn
            .query_row(
                "SELECT value FROM index_state WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let has_vectors_table: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master \
                 WHERE type = 'table' AND name = 'commit_vectors'",
                [],
                |_| Ok(()),
            )
            .is_ok();
        let embeddings_flag: bool = conn
            .query_row(
                "SELECT value FROM index_state WHERE key = 'embeddings_enabled'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let embeddings_enabled = has_vectors_table && embeddings_flag;

        let model: Option<String> = conn
            .query_row(
                "SELECT value FROM index_state WHERE key = 'embeddings.model_name'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok();

        Ok(Self {
            db_path,
            commit_count,
            db_size_bytes,
            schema_version,
            embeddings_enabled,
            model,
            writer_lock,
        })
    }
}

/// Parse `<pid>\n<rfc3339>\n` from the lockfile. Returns `None` when the
/// file is missing, empty, truncated, or any field fails to parse.
fn read_lock_payload(path: &Path) -> Option<WriterLockInfo> {
    let body = fs::read_to_string(path).ok()?;
    let mut lines = body.lines();
    let pid = lines.next()?.trim().parse::<u32>().ok()?;
    let started_at = lines.next()?.trim().to_string();
    if started_at.is_empty() {
        return None;
    }
    Some(WriterLockInfo { pid, started_at })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn read_lock_payload_returns_none_when_missing() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope.lock");
        assert!(read_lock_payload(&missing).is_none());
    }

    #[test]
    fn read_lock_payload_parses_two_line_payload() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.lock");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"4242\n2026-01-01T00:00:00Z\n").unwrap();
        let info = read_lock_payload(&p).unwrap();
        assert_eq!(info.pid, 4242);
        assert!(info.started_at.contains("2026-01-01"));
    }

    #[test]
    fn read_lock_payload_returns_none_when_truncated() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.lock");
        std::fs::write(&p, "4242\n").unwrap();
        assert!(read_lock_payload(&p).is_none());
    }
}

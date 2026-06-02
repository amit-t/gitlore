//! Search connection pool (TDD-001 §2.1).
//!
//! [`SearchConnPool`] wraps a `Mutex<rusqlite::Connection>` opened with
//! `SQLITE_OPEN_READ_ONLY`, `PRAGMA query_only=1`, and
//! `PRAGMA cache_size=-20000` (~20 MiB page cache) for concurrent read
//! access to the SQLite index.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags};

use crate::error::{Error, Result};
use crate::git::GitProvider;
use crate::index::indexer::INDEX_DB_FILENAME;
use crate::index::storage::resolve_index_path;

/// Search connection pool.
///
/// Wraps a single read-only SQLite connection in a `Mutex` for safe
/// concurrent access. The connection is opened with:
///
/// * `SQLITE_OPEN_READ_ONLY` — no write operations allowed
/// * `SQLITE_OPEN_URI` — URI filename support
/// * `SQLITE_OPEN_NO_MUTEX` — rusqlite's internal mutex is disabled since
///   we wrap the connection in our own `Mutex`
///
/// And configured with:
///
/// * `PRAGMA query_only=1` — additional protection against accidental writes
/// * `PRAGMA cache_size=-20000` — ~20 MiB page cache for better read
///   performance
#[derive(Debug)]
pub struct SearchConnPool {
    /// The underlying SQLite connection, wrapped in a Mutex for
    /// thread-safe concurrent access.
    conn: Mutex<Connection>,
}

impl SearchConnPool {
    /// Open a new search connection pool for the repository rooted at
    /// `repo_root`.
    ///
    /// Returns an error if the index does not exist or cannot be opened.
    pub fn open(repo_root: &Path, provider: &dyn GitProvider) -> Result<Self> {
        let db_path = resolve_index_path(repo_root, provider)?
            .path()
            .join(INDEX_DB_FILENAME);

        if !db_path.exists() {
            return Err(Error::IndexNotReady);
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        let conn = Connection::open_with_flags(&db_path, flags)
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        // Set query_only=1 for additional write protection
        conn.pragma_update(None, "query_only", 1)
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        // Set cache_size=-20000 for ~20 MiB page cache
        conn.pragma_update(None, "cache_size", -20000)
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Execute a read-only operation on the underlying connection.
    ///
    /// The callback receives a mutable reference to the connection and
    /// can perform read-only queries. The mutex is held for the duration
    /// of the callback.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use gitlore_core::search::conn_pool::SearchConnPool;
    /// # use gitlore_core::git::cli::GitCliProvider;
    /// # use std::path::Path;
    /// # let provider = GitCliProvider::new(Path::new("/repo"));
    /// # let pool = SearchConnPool::open(Path::new("/repo"), &provider).unwrap();
    /// let count = pool.with_conn(|conn| {
    ///     conn.query_row("SELECT COUNT(*) FROM commits", [], |row| row.get::<_, i64>(0))
    /// }).unwrap();
    /// ```
    pub fn with_conn<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Connection) -> std::result::Result<R, rusqlite::Error>,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|e| Error::Sqlite(format!("Failed to acquire connection lock: {}", e)))?;
        f(&conn).map_err(|e| Error::Sqlite(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitProvider;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// `GitProvider` stub that only implements `common_dir` — every other
    /// method panics so an accidental call is loud rather than silent.
    struct StubProvider {
        common: PathBuf,
        called: AtomicU32,
    }

    impl StubProvider {
        fn new(common: PathBuf) -> Self {
            Self {
                common,
                called: AtomicU32::new(0),
            }
        }
    }

    impl GitProvider for StubProvider {
        fn common_dir(&self) -> Result<PathBuf> {
            self.called.fetch_add(1, Ordering::SeqCst);
            Ok(self.common.clone())
        }
        fn rev_parse(&self, _: &str) -> Result<crate::git::Sha> {
            unimplemented!()
        }
        fn list_refs(&self, _: crate::git::RefScope) -> Result<Vec<crate::git::RefEntry>> {
            unimplemented!()
        }
        fn walk_commits(&self, _: crate::git::WalkRange) -> Result<Vec<crate::git::RawCommit>> {
            unimplemented!()
        }
        fn show(&self, _: &crate::git::Sha, _: crate::git::ShowOpts) -> Result<String> {
            unimplemented!()
        }
        fn check_mailmap(&self, _: &str, _: &str) -> Result<crate::git::MailmapResolved> {
            unimplemented!()
        }
        fn cat_file_exists(&self, _: &crate::git::Sha) -> Result<bool> {
            unimplemented!()
        }
    }

    #[test]
    fn test_open_returns_error_when_index_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        let provider = StubProvider::new(common);
        let result = SearchConnPool::open(tmp.path(), &provider);
        assert!(matches!(result, Err(Error::IndexNotReady)));
    }

    #[test]
    fn test_open_succeeds_with_valid_index() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        let dir = common.join("gitlore");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(INDEX_DB_FILENAME);
        let mut conn = Connection::open(&db_path).unwrap();
        crate::index::migrations::migrate(&mut conn).unwrap();

        let provider = StubProvider::new(common);
        let pool = SearchConnPool::open(tmp.path(), &provider).unwrap();
        let count: i64 = pool
            .with_conn(|conn| conn.query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0)))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_with_conn_executes_read_only_query() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        let dir = common.join("gitlore");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(INDEX_DB_FILENAME);
        let mut conn = Connection::open(&db_path).unwrap();
        crate::index::migrations::migrate(&mut conn).unwrap();

        // Insert a test commit with all required fields
        conn.execute(
            "INSERT INTO commits (sha, author_name, author_email, committer_name, committer_email, authored_at, committed_at, subject, body, indexed_at, updated_at) VALUES ('abc123', 'Test', 'test@example.com', 'Test', 'test@example.com', 0, 0, 'Test', '', 0, 0)",
            [],
        )
        .unwrap();

        let provider = StubProvider::new(common);
        let pool = SearchConnPool::open(tmp.path(), &provider).unwrap();
        let count: i64 = pool
            .with_conn(|conn| conn.query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0)))
            .unwrap();
        assert_eq!(count, 1);
    }
}

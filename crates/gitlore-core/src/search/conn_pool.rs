//! Read-only SQLite connection pool for search (TDD-001 §2.1 / grill #17).
//!
//! `SearchConnPool` wraps a single `rusqlite::Connection` opened in
//! read-only mode behind a `Mutex`. A single connection is sufficient for
//! M4: the TUI (M5) will revisit if profiling shows contention.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags};

use crate::error::{Error, Result};

/// A single read-only SQLite connection shared across search calls via a mutex.
///
/// Opened with:
/// * `SQLITE_OPEN_READ_ONLY` — prevents any writes.
/// * `PRAGMA query_only = 1` — belt-and-suspenders guard against writes.
/// * `PRAGMA cache_size = -20000` — ~20 MiB page cache for warm queries.
pub struct SearchConnPool {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for SearchConnPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchConnPool")
            .field("conn", &"<rusqlite::Connection>")
            .finish()
    }
}

impl SearchConnPool {
    /// Open a read-only pool for the SQLite database at `path`.
    ///
    /// Opens with `SQLITE_OPEN_READ_WRITE` so SQLite can create the `-shm`
    /// WAL-index file that WAL-mode databases require even for readers. Writes
    /// are blocked at the SQL layer via `PRAGMA query_only = 1`.  Opening with
    /// `SQLITE_OPEN_READ_ONLY` would cause "disk I/O error" on WAL-mode
    /// databases when the `-shm` file has not yet been created.
    pub fn open(path: &Path) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn =
            Connection::open_with_flags(path, flags).map_err(|e| Error::Sqlite(e.to_string()))?;

        // Belt-and-suspenders: prevent any accidental writes.
        conn.execute_batch("PRAGMA query_only = 1; PRAGMA cache_size = -20000;")
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Execute a closure with a locked reference to the inner connection.
    ///
    /// Returns `Error::Sqlite` if the mutex is poisoned (should never happen
    /// in practice since the connection is read-only).
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let guard = self
            .conn
            .lock()
            .map_err(|_| Error::Sqlite("connection mutex poisoned".into()))?;
        f(&guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn make_writable_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();
    }

    #[test]
    fn opens_read_only_conn() {
        let f = NamedTempFile::new().unwrap();
        make_writable_db(f.path());

        let pool = SearchConnPool::open(f.path()).unwrap();
        let result = pool
            .with_conn(|c| {
                let n: i64 = c
                    .query_row("SELECT x FROM t", [], |r| r.get(0))
                    .map_err(|e| Error::Sqlite(e.to_string()))?;
                Ok(n)
            })
            .unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn write_is_rejected() {
        let f = NamedTempFile::new().unwrap();
        make_writable_db(f.path());

        let pool = SearchConnPool::open(f.path()).unwrap();
        let result = pool.with_conn(|c| {
            c.execute("INSERT INTO t VALUES (2)", [])
                .map_err(|e| Error::Sqlite(e.to_string()))
        });
        assert!(result.is_err(), "write should have been rejected");
    }
}

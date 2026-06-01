//! Writer lock + WAL checkpoint helpers (TDD-000 §2.2, SPEC-001 §3.4/§4.5,
//! ADR-004).
//!
//! gitlore allows arbitrarily many concurrent readers against the SQLite
//! index, but only one writer at a time. The lockfile lives next to the
//! database (e.g. `<index-dir>/index.lock`) and combines two signals:
//!
//! 1. **Advisory OS lock** via `fs2::FileExt::lock_exclusive` /
//!    `try_lock_exclusive`. This is the source of truth — the kernel
//!    enforces mutual exclusion across processes, even across crashes.
//! 2. **`<pid>\n<rfc3339>\n` payload** stamped into the file body. The
//!    payload is purely informational: it lets a contending writer report
//!    *which* PID holds the lock and *when* it grabbed it without having
//!    to walk `/proc`. It is also how a Wait-mode contender detects a
//!    stale lockfile (holder exited but the file lingered because the OS
//!    lock survived a `kill -9`-style crash on macOS where `flock` state
//!    can outlive the fd briefly) and reclaims it via `kill -0`.
//!
//! ## Acquisition modes
//!
//! * [`LockMode::NoWait`] — single `try_lock_exclusive`. On contention
//!   parses the lockfile and returns [`Error::LockContention`] with the
//!   recorded `held_pid` / `started_at` so the caller can render
//!   `another writer (pid=…) is holding the lock since …`.
//! * [`LockMode::Wait`] — first attempts `try_lock_exclusive` so it can
//!   stale-check the holder's PID via `kill -0`. If the holder is dead
//!   (`ESRCH`) the stale lockfile is removed and the lock is retried
//!   exactly once. If the holder is alive, the call falls through to
//!   `lock_exclusive` (kernel-level wait) so the contender simply blocks
//!   until the holder drops the lock.
//!
//! ## Drop semantics
//!
//! `Drop` releases the OS lock via `fs2::FileExt::unlock` and best-effort
//! removes the lockfile. Removal failures are swallowed — by the time we
//! reach `Drop` the next writer may already have grabbed the file via the
//! Wait-mode stale-reclaim path, and racing it would either delete the
//! new holder's payload or fail with `ENOENT`. Either way the OS lock
//! release is the only step that matters for correctness; the file body
//! is a diagnostic aid.
//!
//! `Drop` runs even on panic (verified by
//! `tests/concurrency_two_indexers.rs`), so the lockfile cannot remain
//! held by a process that has unwound past the [`WriterLock`] binding.
//!
//! ## WAL checkpoint
//!
//! [`wal_checkpoint_if_large`] is a sibling helper rather than a method on
//! [`WriterLock`] so callers can wire it into the index-open path
//! (`open_index(...)` lands at M3-6). When `<db-path>-wal` grows past
//! `threshold_bytes` (default 100 MiB per AC-CON-3) we run
//! `PRAGMA wal_checkpoint(TRUNCATE)` to flush committed pages back into
//! the main database file and shrink the WAL on disk. The check is cheap
//! (one `stat`); the checkpoint runs only when the WAL is actually
//! oversized.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use rusqlite::Connection;

use crate::error::{Error, Result};

/// Default WAL size threshold above which [`wal_checkpoint_if_large`]
/// triggers `PRAGMA wal_checkpoint(TRUNCATE)`. 100 MiB per AC-CON-3.
pub const DEFAULT_WAL_CHECKPOINT_THRESHOLD_BYTES: u64 = 100 * 1024 * 1024;

/// How [`acquire`] should behave when another writer already holds the
/// lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Block (kernel-level) until the holder drops the lock. The Wait
    /// path first probes the existing lockfile via `kill -0` so a stale
    /// holder is reclaimed without blocking forever.
    Wait,
    /// Fail fast with [`Error::LockContention`] when the lock is held.
    NoWait,
}

/// An RAII handle to the index writer lock.
///
/// Dropping a [`WriterLock`] releases the OS lock and best-effort removes
/// the lockfile. The file handle is kept alive for the entire lock's
/// lifetime; closing it would also release the OS lock on Unix, so
/// callers must hold this struct for the duration of the critical
/// section.
#[derive(Debug)]
pub struct WriterLock {
    file: File,
    path: PathBuf,
}

impl WriterLock {
    /// Path of the lockfile on disk.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        // Release the OS lock first. If unlock fails we still try the
        // file removal — the OS will release the lock on close anyway.
        let _ = FileExt::unlock(&self.file);
        // Best-effort removal: another writer may have already reclaimed
        // the lockfile via the Wait-mode stale path, in which case
        // ENOENT is the expected outcome.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Acquire the writer lock at `path` honouring `mode`.
///
/// Always opens (or creates) `path` with read+write permissions so the
/// `<pid>\n<rfc3339>\n` payload can be stamped after the OS lock is
/// granted. See the module docs for the full state machine.
///
/// # Errors
///
/// * [`Error::LockContention`] — `NoWait` mode and another process holds
///   the lock (payload parsed where readable).
/// * [`Error::Io`] — opening the lockfile failed, or the blocking
///   `lock_exclusive` call returned a non-`WouldBlock` error.
pub fn acquire(path: &Path, mode: LockMode) -> Result<WriterLock> {
    // Ensure the parent dir exists; callers normally pre-create the
    // index dir but a missing parent here would otherwise present as a
    // bare ENOENT with no breadcrumb.
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let file = open_lockfile(path)?;
    let acquired = match mode {
        LockMode::NoWait => acquire_nowait(&file, path)?,
        LockMode::Wait => acquire_wait(file, path)?,
    };
    stamp_payload(&acquired.file)?;
    Ok(acquired)
}

/// Open `path` with read+write+create so both lock and payload writes
/// succeed against either a fresh or an existing file.
fn open_lockfile(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Try once. On `WouldBlock` parse the lockfile and surface
/// [`Error::LockContention`] so the caller can render the holder's PID.
fn acquire_nowait(file: &File, path: &Path) -> Result<WriterLock> {
    match FileExt::try_lock_exclusive(file) {
        Ok(()) => Ok(WriterLock {
            // Move a clone of the fd into the handle so the original `&File`
            // stays valid for the caller's payload write below.
            file: file.try_clone()?,
            path: path.to_path_buf(),
        }),
        Err(e) if is_would_block(&e) => {
            let (held_pid, started_at) = parse_lockfile(path).unwrap_or((None, None));
            Err(Error::LockContention {
                lock_path: path.to_path_buf(),
                held_pid,
                started_at,
            })
        }
        Err(e) => Err(Error::Io(e)),
    }
}

/// Wait-mode acquisition: probe the lock once, reclaim a stale holder if
/// possible, then fall back to a blocking `lock_exclusive` call when the
/// holder is alive.
fn acquire_wait(file: File, path: &Path) -> Result<WriterLock> {
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            return Ok(WriterLock {
                file,
                path: path.to_path_buf(),
            });
        }
        Err(e) if !is_would_block(&e) => return Err(Error::Io(e)),
        Err(_) => {}
    }

    // Try to learn who holds it. A malformed lockfile parses to `None`
    // and falls through to the blocking branch — we never reclaim a
    // file we cannot identify the owner of.
    if let Some(holder_pid) = parse_lockfile(path).ok().and_then(|(p, _)| p) {
        match kill(Pid::from_raw(holder_pid as i32), None) {
            Err(Errno::ESRCH) => {
                // Stale holder. Drop our handle (releases any phantom
                // lock state on the old fd), remove the file, reopen,
                // and retry exactly once.
                drop(file);
                std::fs::remove_file(path)?;
                let retried = open_lockfile(path)?;
                FileExt::try_lock_exclusive(&retried).map_err(Error::Io)?;
                return Ok(WriterLock {
                    file: retried,
                    path: path.to_path_buf(),
                });
            }
            Ok(()) | Err(_) => {
                // Holder is alive (or signal probe failed in a way we
                // cannot interpret as "dead"); block on the kernel.
            }
        }
    }

    FileExt::lock_exclusive(&file).map_err(Error::Io)?;
    Ok(WriterLock {
        file,
        path: path.to_path_buf(),
    })
}

/// Truncate the lockfile and write `<pid>\n<rfc3339>\n` so contenders
/// can identify the holder.
fn stamp_payload(file: &File) -> Result<()> {
    let pid = std::process::id();
    let when = chrono::Utc::now().to_rfc3339();
    let payload = format!("{pid}\n{when}\n");

    // `set_len(0)` is the portable truncate; we then seek to 0 so a
    // re-acquire-then-stamp does not leave stale bytes past `payload.len()`.
    let mut handle = file;
    handle.set_len(0)?;
    handle.seek(SeekFrom::Start(0))?;
    handle.write_all(payload.as_bytes())?;
    handle.flush()?;
    Ok(())
}

/// Read `<pid>\n<rfc3339>\n` from `path`. Missing fields parse to
/// `None`; malformed or unreadable files parse to `(None, None)` so the
/// caller can still surface a contention error.
fn parse_lockfile(path: &Path) -> io::Result<(Option<u32>, Option<String>)> {
    let mut buf = String::new();
    File::open(path)?.read_to_string(&mut buf)?;
    let mut lines = buf.lines();
    let pid = lines.next().and_then(|s| s.trim().parse::<u32>().ok());
    let started_at = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok((pid, started_at))
}

/// `true` when the OS reported "would block" for a non-blocking lock
/// attempt. `fs2` maps both `EWOULDBLOCK` and `EAGAIN` to
/// `ErrorKind::WouldBlock`.
fn is_would_block(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::WouldBlock
}

/// Run `PRAGMA wal_checkpoint(TRUNCATE)` against `conn` iff
/// `<db_path>-wal` is larger than `threshold_bytes` on disk.
///
/// Returns `Ok(true)` when a checkpoint actually ran, `Ok(false)`
/// otherwise (WAL missing or below the threshold). Errors propagate as
/// [`Error::Io`] for the stat failure and [`Error::Sqlite`] for the
/// PRAGMA execution failure.
///
/// Wire this into the index-open path so a long-lived writer cannot
/// inflate the WAL indefinitely. M3-6 calls it; M3-3 only exposes the
/// helper.
pub fn wal_checkpoint_if_large(
    conn: &Connection,
    db_path: &Path,
    threshold_bytes: u64,
) -> Result<bool> {
    let wal_path = wal_sibling(db_path);
    let size = match std::fs::metadata(&wal_path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(Error::Io(e)),
    };
    if size <= threshold_bytes {
        return Ok(false);
    }
    // `wal_checkpoint(TRUNCATE)` returns a (busy, log_pages, ckpt_pages)
    // row; we only care that the statement ran without an error.
    conn.pragma_update(None, "wal_checkpoint", "TRUNCATE")
        .map_err(|e| Error::Sqlite(format!("wal_checkpoint(TRUNCATE) failed: {e}")))?;
    Ok(true)
}

/// `<db>-wal` sibling path SQLite uses for the WAL file in journal mode
/// `WAL`.
fn wal_sibling(db_path: &Path) -> PathBuf {
    let mut name = db_path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push("-wal");
    db_path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockmode_is_copy() {
        let m = LockMode::Wait;
        let _n = m;
        let _o = m;
    }

    #[test]
    fn nowait_acquires_fresh_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.lock");
        let guard = acquire(&path, LockMode::NoWait).unwrap();
        assert!(path.exists());
        let body = std::fs::read_to_string(&path).unwrap();
        let mut lines = body.lines();
        assert_eq!(
            lines.next().unwrap().parse::<u32>().unwrap(),
            std::process::id()
        );
        let stamp = lines.next().unwrap();
        assert!(stamp.contains('T'), "rfc3339 should contain T: {stamp}");
        drop(guard);
        assert!(!path.exists(), "Drop best-effort removes the lockfile");
    }

    #[test]
    fn wait_acquires_fresh_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.lock");
        let guard = acquire(&path, LockMode::Wait).unwrap();
        assert_eq!(guard.path(), path.as_path());
    }

    #[test]
    fn nowait_returns_contention_when_held() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.lock");
        let _held = acquire(&path, LockMode::NoWait).unwrap();
        let err = acquire(&path, LockMode::NoWait).unwrap_err();
        match err {
            Error::LockContention {
                lock_path,
                held_pid,
                started_at,
            } => {
                assert_eq!(lock_path, path);
                assert_eq!(held_pid, Some(std::process::id()));
                assert!(started_at.is_some(), "payload must round-trip");
            }
            other => panic!("expected LockContention, got {other:?}"),
        }
    }

    #[test]
    fn parse_lockfile_handles_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.lock");
        assert!(parse_lockfile(&missing).is_err());
    }

    #[test]
    fn parse_lockfile_handles_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("empty.lock");
        std::fs::write(&p, "").unwrap();
        let (pid, when) = parse_lockfile(&p).unwrap();
        assert!(pid.is_none());
        assert!(when.is_none());
    }

    #[test]
    fn parse_lockfile_handles_pid_only() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("pid.lock");
        std::fs::write(&p, "1234\n").unwrap();
        let (pid, when) = parse_lockfile(&p).unwrap();
        assert_eq!(pid, Some(1234));
        assert!(when.is_none());
    }

    #[test]
    fn wal_sibling_appends_wal_suffix() {
        let p = Path::new("/tmp/foo/bar.db");
        assert_eq!(wal_sibling(p), Path::new("/tmp/foo/bar.db-wal"));
    }

    #[test]
    fn wal_checkpoint_if_large_returns_false_when_no_wal() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("idx.db");
        let conn = Connection::open(&db).unwrap();
        let ran = wal_checkpoint_if_large(&conn, &db, 1024).unwrap();
        assert!(!ran, "no WAL file → no checkpoint");
    }

    #[test]
    fn wal_checkpoint_if_large_returns_false_when_under_threshold() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("idx.db");
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        // Force WAL creation by performing a write.
        conn.execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();
        let wal = wal_sibling(&db);
        let size = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        let ran = wal_checkpoint_if_large(&conn, &db, size + 1).unwrap();
        assert!(!ran);
    }

    #[test]
    fn default_threshold_is_100_mib() {
        assert_eq!(DEFAULT_WAL_CHECKPOINT_THRESHOLD_BYTES, 100 * 1024 * 1024);
    }
}

//! AC-CON-3: `wal_checkpoint_if_large` triggers when `<db>-wal` exceeds
//! the threshold (TDD-000 §2.2, default 100 MiB).
//!
//! Generating 100 MiB of real WAL traffic through INSERT statements is
//! slow and flaky in CI. The contract under test is "stat the WAL, run
//! `PRAGMA wal_checkpoint(TRUNCATE)` when it is too large", so we
//! pre-bloat the WAL with `set_len(101 << 20)` on the same connection
//! that opened WAL mode. SQLite's TRUNCATE checkpoint then rewinds the
//! file via `ftruncate(0)`, so the post-call size collapses regardless
//! of how many real pages were in it.

use std::fs::{File, OpenOptions};
use std::path::Path;

use rusqlite::Connection;

use gitlore_core::index::lock::{wal_checkpoint_if_large, DEFAULT_WAL_CHECKPOINT_THRESHOLD_BYTES};

/// 101 MiB — one MiB past the default threshold so the helper trips.
const BLOATED_WAL_BYTES: u64 = 101 * 1024 * 1024;

fn wal_len(db: &Path) -> u64 {
    let wal = sibling(db, "-wal");
    std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0)
}

fn sibling(db: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = db.file_name().unwrap().to_os_string();
    name.push(suffix);
    db.with_file_name(name)
}

#[test]
fn returns_true_and_truncates_oversize_wal() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("idx.db");

    // Open the DB in WAL mode and do one trivial write so SQLite creates
    // a real, header-valid `<db>-wal` file. Keep the connection open
    // through the entire test — dropping/reopening it would force SQLite
    // to re-read the WAL header and could truncate our bloated tail.
    let conn = Connection::open(&db).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1);")
        .unwrap();

    // Bloat the WAL to 101 MiB via `set_len`. The file is sparse on
    // every supported filesystem; physical disk usage stays tiny.
    // `truncate=false` keeps the existing header bytes intact.
    let wal_path = sibling(&db, "-wal");
    let wal = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&wal_path)
        .unwrap();
    wal.set_len(BLOATED_WAL_BYTES).unwrap();
    wal.sync_all().unwrap();
    drop(wal);
    let pre = wal_len(&db);
    assert!(
        pre >= BLOATED_WAL_BYTES,
        "WAL must reach bloated size; got {pre}"
    );

    let ran = wal_checkpoint_if_large(&conn, &db, DEFAULT_WAL_CHECKPOINT_THRESHOLD_BYTES).unwrap();
    assert!(ran, "checkpoint must run when WAL > 100 MiB (pre={pre})");

    let after = wal_len(&db);
    assert!(
        after < 1024 * 1024,
        "TRUNCATE checkpoint must shrink WAL below 1 MiB; got {after} bytes"
    );
}

#[test]
fn returns_false_when_wal_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("idx.db");
    // Touch the DB but do not open WAL mode, so `<db>-wal` does not
    // exist. The helper must short-circuit to Ok(false).
    File::create(&db).unwrap();
    let conn = Connection::open(&db).unwrap();
    let ran = wal_checkpoint_if_large(&conn, &db, DEFAULT_WAL_CHECKPOINT_THRESHOLD_BYTES).unwrap();
    assert!(!ran);
}

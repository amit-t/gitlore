//! AC-CON-2: Wait-mode reclaim of a stale lockfile (TDD-000 §2.2).
//!
//! When a previous writer crashed and left its `<pid>\n<rfc3339>\n`
//! payload behind, the next writer in Wait mode must:
//!   1. Detect the holder is gone via `kill -0` returning `ESRCH`.
//!   2. Remove the stale file.
//!   3. Re-acquire so the lockfile now records the new writer's PID.
//!
//! We forge a stale payload by writing a deliberately impossible PID
//! (`99_999_999` — well above the per-process `pid_max` on every
//! supported platform) and a frozen RFC-3339 timestamp. The file is
//! NOT locked at the OS level: a real stale lockfile is one where the
//! writer crashed after release-but-before-remove, or where remove
//! lost a race. Either way the OS lock is free and only the payload
//! lingers.

use std::fs;
use std::path::PathBuf;

use gitlore_core::index::lock::{acquire, LockMode};

#[test]
fn wait_reclaims_lockfile_when_recorded_pid_is_dead() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("index.lock");

    // Stamp a stale payload. The file is created without locking it.
    fs::write(&path, "99999999\n2026-01-01T00:00:00Z\n").unwrap();
    assert!(path.exists());

    // Wait-mode acquire must reclaim instead of blocking forever.
    let guard = acquire(&path, LockMode::Wait).expect("wait-mode reclaim after ESRCH");

    // After reclaim the file body identifies us.
    let body = fs::read_to_string(&path).unwrap();
    let mut lines = body.lines();
    let pid: u32 = lines
        .next()
        .expect("pid line")
        .trim()
        .parse()
        .expect("pid parses");
    assert_eq!(
        pid,
        std::process::id(),
        "stale lockfile must be replaced with our PID after reclaim"
    );
    let when = lines.next().expect("rfc3339 line");
    assert!(
        when.contains('T'),
        "stamp must look like rfc3339, got `{when}`"
    );
    drop(guard);
}

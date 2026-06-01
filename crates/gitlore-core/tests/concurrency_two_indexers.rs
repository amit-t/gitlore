//! AC-CON-1 + AC-CON-2: two-writer mutual exclusion (TDD-000 §2.2,
//! SPEC-001 §3.4 / §4.5).
//!
//! Asserts that `WriterLock` enforces "one writer at a time" across two
//! threads contending for the same lockfile, and that `LockMode::NoWait`
//! fails fast with `Error::LockContention` while the lock is held. Also
//! verifies the `Drop` impl releases the lock even when the holder
//! panics — `std::panic::catch_unwind` lets the second thread observe
//! the release without bringing the test process down.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use gitlore_core::error::Error;
use gitlore_core::index::lock::{acquire, LockMode};

/// Helper: per-test lockfile path inside a fresh tempdir we leak for the
/// test's lifetime. Returning the `TempDir` would force every call site
/// to bind it; leaking via `into_path` keeps test bodies tight without
/// risking premature deletion.
fn fresh_lock_path() -> (PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.lock");
    (path, dir)
}

#[test]
fn wait_mode_serialises_two_threads() {
    let (path, _dir) = fresh_lock_path();
    let path_a = path.clone();
    let path_b = path.clone();

    let (tx, rx) = mpsc::channel::<()>();

    // Thread A: take the lock, signal it has it, hold for ~200 ms.
    let a = thread::spawn(move || {
        let guard = acquire(&path_a, LockMode::Wait).expect("A: acquire");
        tx.send(()).expect("signal");
        thread::sleep(Duration::from_millis(200));
        // Explicit drop so the lock is released exactly here, not at the
        // end of the closure (which would be identical here but is
        // clearer for the test reader).
        drop(guard);
    });

    // Wait until A actually holds the lock before B even tries.
    rx.recv().unwrap();

    let start = Instant::now();
    let b = thread::spawn(move || {
        let guard = acquire(&path_b, LockMode::Wait).expect("B: acquire after A drop");
        let elapsed = start.elapsed();
        drop(guard);
        elapsed
    });

    a.join().expect("A finished");
    let elapsed = b.join().expect("B finished");

    assert!(
        elapsed >= Duration::from_millis(100),
        "B should block until A released; took only {elapsed:?}"
    );
}

#[test]
fn nowait_mode_fails_fast_when_held() {
    let (path, _dir) = fresh_lock_path();
    let held = acquire(&path, LockMode::NoWait).expect("first acquire");

    let start = Instant::now();
    let err = acquire(&path, LockMode::NoWait).expect_err("second NoWait must fail");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(100),
        "NoWait must fail immediately, took {elapsed:?}"
    );
    match err {
        Error::LockContention {
            lock_path,
            held_pid,
            started_at,
        } => {
            assert_eq!(lock_path, path);
            assert_eq!(held_pid, Some(std::process::id()));
            assert!(
                started_at.is_some(),
                "lockfile payload must record an rfc3339 timestamp"
            );
        }
        other => panic!("expected LockContention, got {other:?}"),
    }
    drop(held);
}

#[test]
fn drop_releases_lock_on_panic() {
    let (path, _dir) = fresh_lock_path();
    let path_for_panic = path.clone();

    // `catch_unwind` unwinds the closure but lets the test process keep
    // running. The lock guard inside the closure must `Drop` during
    // unwind, releasing the OS lock for the follow-up acquire below.
    let outcome = std::panic::catch_unwind(move || {
        let _guard = acquire(&path_for_panic, LockMode::NoWait).expect("panic-path acquire");
        panic!("intentional panic to trigger Drop");
    });
    assert!(outcome.is_err(), "closure must have panicked");

    // If Drop ran correctly, the lock is free and NoWait succeeds.
    let after = acquire(&path, LockMode::NoWait)
        .expect("Drop must have released the lock during panic unwind");
    drop(after);
}

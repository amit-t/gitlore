//! AC-IDX-4: resume cleanly after `kill -9` mid-walk (M3-6, TDD-000 §2.2).
//!
//! Build a 1500-commit fixture, fork the `examples/run_initial` harness
//! against it, poll the harness's stdout until partial progress is
//! observed, then `kill -9` the child. Re-run the harness against the
//! same repo + same on-disk index. The second run must succeed, finish
//! the remaining work, and the final state must contain no duplicate
//! `commits.sha` rows (the resume path must be exactly-once thanks to
//! `INSERT ... ON CONFLICT DO UPDATE`).
//!
//! Marked `#[ignore]` because generating 1500 commits via
//! `git commit` loops costs ~30 s on a warm laptop. The full run lives
//! behind `cargo test -- --ignored`, and the assertion lands again at
//! M3-7 once the private fixture-tarball wiring exists.
//
// TODO: pre-built fixture lands at M3-7 once private-fixture wiring exists.

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rusqlite::Connection;

use gitlore_core::index::indexer::INDEX_DB_FILENAME;
use gitlore_core::index::storage::resolve_index_path;

use common::Fixture;

const FIXTURE_COMMITS: usize = 1500;

fn db_path(fx: &Fixture) -> std::path::PathBuf {
    let provider = gitlore_core::git::cli::GitCliProvider::new(fx.repo.clone());
    let loc = resolve_index_path(&fx.repo, &provider).unwrap();
    loc.path().join(INDEX_DB_FILENAME)
}

fn build_fixture() -> Fixture {
    let fx = Fixture::init();
    // Cheap-but-not-free synthetic history: `git commit --allow-empty`
    // with throwaway file writes. Plain shell loop in-process is fine
    // for a 1500-commit fixture (~30s on a warm laptop).
    for i in 0..FIXTURE_COMMITS {
        let path = format!("file_{i}.txt");
        let _ = fx.commit(&path, &format!("rev {i}"), &format!("c{i}"));
    }
    fx
}

fn spawn_harness(repo: &std::path::Path) -> std::process::Child {
    Command::new(env!("CARGO"))
        .args([
            "run",
            "--quiet",
            "--example",
            "run_initial",
            "--manifest-path",
        ])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .arg("--")
        .arg(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn run_initial example")
}

#[test]
#[ignore]
fn resume_after_sigkill_yields_no_duplicates() {
    let fx = build_fixture();

    // First run: kill once we see partial progress.
    let mut child = spawn_harness(&fx.repo);
    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut saw_progress = false;
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        line.clear();
        let read = reader.read_line(&mut line).unwrap_or(0);
        if read == 0 {
            break;
        }
        if line.starts_with("PROGRESS ") {
            // Wait until we've seen at least ~10 commits committed so
            // the chunk-boundary watermark has been flushed at least
            // once.
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(processed) = parts[1].parse::<u64>() {
                    if processed >= 10 {
                        saw_progress = true;
                        break;
                    }
                }
            }
        }
    }
    assert!(saw_progress, "harness never reported progress");
    let _ = child.kill();
    let _ = child.wait();

    // Re-run the harness against the same repo + same on-disk index.
    let second = spawn_harness(&fx.repo);
    let out = second.wait_with_output().expect("second harness wait");
    assert!(
        out.status.success(),
        "resume run failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Inspect the persisted index.
    let conn = Connection::open(db_path(&fx)).unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        total, FIXTURE_COMMITS as i64,
        "final commit count = fixture size"
    );
    let dup: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (SELECT sha FROM commits GROUP BY sha HAVING COUNT(*) > 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dup, 0, "no duplicate commit SHA rows after resume");
}

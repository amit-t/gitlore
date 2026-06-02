//! Tiny harness binary: `cargo run --example run_initial -- <repo>`.
//!
//! Spawned by the M3-6 integration tests that need a separate process so
//! they can `kill -9` it mid-walk and verify
//! [`gitlore_core::index::indexer::Indexer::run_initial`] is resumable
//! (AC-IDX-4). Once M3-7 lands the proper `gitlore index` subcommand the
//! tests will switch to invoking the real binary; until then this
//! example stands in.

use std::path::PathBuf;
use std::process::ExitCode;

use gitlore_core::index::indexer::Indexer;
use gitlore_core::index::lock::LockMode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(repo) = args.next() else {
        eprintln!("usage: run_initial <repo-root>");
        return ExitCode::from(2);
    };
    let repo_root = PathBuf::from(repo);
    let mut indexer = match Indexer::open(&repo_root, LockMode::NoWait) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("open failed: {e}");
            return ExitCode::from(1);
        }
    };
    let report = match indexer.run_initial(&mut |processed, total| {
        // Print one progress line per commit so the parent test can poll
        // for partial progress and SIGKILL the child mid-walk.
        println!("PROGRESS {processed} {total}");
    }) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("run_initial failed: {e}");
            return ExitCode::from(1);
        }
    };
    println!(
        "DONE indexed={} total={} refs={} ms={}",
        report.commits_indexed, report.commits_total, report.ref_count, report.duration_ms
    );
    ExitCode::SUCCESS
}

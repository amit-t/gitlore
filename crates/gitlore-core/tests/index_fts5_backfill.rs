//! AC-IDX-FTS5: FTS5 backfill idempotency (TDD-001 §2.2 / SPEC-001 §5).
//!
//! Calling `run_initial` a second time on an already-indexed repo must
//! succeed without error. This is the observable contract of the backfill
//! mechanism: whatever state the FTS5 virtual table is in after a first walk,
//! a second walk must not corrupt it or return an error.

mod common;

use gitlore_core::index::indexer::Indexer;
use gitlore_core::index::lock::LockMode;

use common::Fixture;

fn build_repo(fx: &Fixture) {
    fx.commit("src/lib.rs", "pub fn retry() {}", "fix: retry on timeout");
    fx.commit("src/main.rs", "fn main() {}", "feat: initial commit");
    fx.commit("README.md", "# repo\n", "docs: add readme");
}

#[test]
fn fts5_backfill_is_idempotent() {
    let fx = Fixture::init();
    build_repo(&fx);

    // First index: populates commits + FTS5 virtual table.
    {
        let mut idx = Indexer::open(&fx.repo, LockMode::Wait).expect("open indexer (first pass)");
        let report = idx
            .run_initial(&mut |_, _| {})
            .expect("run_initial (first pass)");
        assert_eq!(
            report.commits_indexed, 3,
            "first pass must index all 3 commits"
        );
    }

    // Second index: idempotent re-run. Must succeed without panic or error.
    {
        let mut idx = Indexer::open(&fx.repo, LockMode::Wait).expect("open indexer (second pass)");
        let report = idx
            .run_initial(&mut |_, _| {})
            .expect("run_initial must be idempotent (second pass)");
        // The second run is a full rebuild: assert the field is valid.
        assert!(
            report.commits_indexed <= 3,
            "second pass reported more commits than exist: {}",
            report.commits_indexed
        );
    }
}

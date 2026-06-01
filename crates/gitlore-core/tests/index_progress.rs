//! AC-IDX-6: the progress callback fires at least N times for an
//! N-commit walk (M3-6, TDD-000 §2.2).
//!
//! Fixture: 10 commits on `main`. Run `run_initial` with a callback
//! that records every `(processed, total)` pair. Assert at least 10
//! ticks fired and that the recorded values are monotonically
//! non-decreasing.

mod common;

use gitlore_core::index::indexer::Indexer;
use gitlore_core::index::lock::LockMode;

use common::Fixture;

#[test]
fn progress_callback_fires_at_least_once_per_commit() {
    let fx = Fixture::init();
    for i in 1..=10 {
        fx.commit(
            &format!("src/f_{i}.rs"),
            &format!("// {i}"),
            &format!("feat: c{i}"),
        );
    }

    let mut ticks: Vec<(u64, u64)> = Vec::new();
    let mut indexer = Indexer::open(&fx.repo, LockMode::NoWait).expect("open");
    let report = indexer
        .run_initial(&mut |processed, total| {
            ticks.push((processed, total));
        })
        .expect("run_initial");

    assert_eq!(report.commits_indexed, 10);
    assert!(
        ticks.len() >= 10,
        "AC-IDX-6: progress must fire ≥10 times for 10-commit walk; got {}",
        ticks.len()
    );

    // `processed` is monotonically non-decreasing.
    let mut prev = 0u64;
    for (processed, _total) in &ticks {
        assert!(
            *processed >= prev,
            "progress.processed went backwards: prev={prev}, now={processed}"
        );
        prev = *processed;
    }

    // The final tick reaches the total commits count.
    let (final_proc, final_total) = *ticks.last().unwrap();
    assert_eq!(final_proc, 10);
    assert_eq!(final_total, 10);
}

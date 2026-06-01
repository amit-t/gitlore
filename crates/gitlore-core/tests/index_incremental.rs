//! AC-IDX-2 + AC-IDX-3: incremental pass is a sub-second no-op when no
//! refs moved, and walks exactly the new commits when they did
//! (M3-6, TDD-000 §2.2).
//!
//! Fixture: 5 commits on `main`, run initial. Then run incremental on
//! the unchanged repo, assert zero new commits in < 1 second. Then add
//! three more commits and re-run incremental; assert exactly three
//! commits walked and the watermark advanced to the new tip.

mod common;

use std::time::{Duration, Instant};

use rusqlite::Connection;

use gitlore_core::index::indexer::{Indexer, INDEX_DB_FILENAME, WATERMARK_KEY};
use gitlore_core::index::lock::LockMode;
use gitlore_core::index::storage::resolve_index_path;

use common::Fixture;

fn db_conn(fx: &Fixture) -> Connection {
    let provider = gitlore_core::git::cli::GitCliProvider::new(fx.repo.clone());
    let loc = resolve_index_path(&fx.repo, &provider).unwrap();
    let path = loc.path().join(INDEX_DB_FILENAME);
    Connection::open(path).unwrap()
}

#[test]
fn incremental_is_noop_on_unchanged_repo_then_walks_new_commits() {
    let fx = Fixture::init();
    for i in 1..=5 {
        fx.commit(
            &format!("src/f_{i}.rs"),
            &format!("// {i}"),
            &format!("feat: c{i}"),
        );
    }

    let initial_report = {
        let mut indexer = Indexer::open(&fx.repo, LockMode::NoWait).expect("open");
        indexer.run_initial(&mut |_, _| {}).expect("run_initial")
    };
    assert_eq!(initial_report.commits_indexed, 5);

    // -------- AC-IDX-2: incremental no-op completes within 1 second.
    let main_head_before = fx.head();
    let elapsed_noop = {
        let mut indexer = Indexer::open(&fx.repo, LockMode::NoWait).expect("open");
        let start = Instant::now();
        let report = indexer
            .run_incremental(&mut |_, _| {})
            .expect("incremental");
        let elapsed = start.elapsed();
        assert_eq!(
            report.commits_indexed, 0,
            "incremental on unchanged repo indexes 0 commits"
        );
        elapsed
    };
    assert!(
        elapsed_noop < Duration::from_secs(1),
        "AC-IDX-2: incremental no-op must complete in <1s, took {elapsed_noop:?}"
    );

    // -------- AC-IDX-3: add 3 new commits, re-run incremental.
    fx.commit("src/f_6.rs", "// 6", "feat: c6");
    fx.commit("src/f_7.rs", "// 7", "feat: c7");
    fx.commit("src/f_8.rs", "// 8", "feat: c8");
    let main_head_after = fx.head();
    assert_ne!(main_head_after, main_head_before, "fixture grew");

    let report = {
        let mut indexer = Indexer::open(&fx.repo, LockMode::NoWait).expect("open");
        indexer
            .run_incremental(&mut |_, _| {})
            .expect("incremental")
    };
    assert_eq!(
        report.commits_indexed, 3,
        "incremental walks exactly the new commits"
    );

    let conn = db_conn(&fx);
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 8, "8 commits indexed in total");

    let watermark_raw: String = conn
        .query_row(
            "SELECT value FROM index_state WHERE key = ?1",
            [WATERMARK_KEY],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&watermark_raw).expect("watermark JSON");
    let advanced = parsed
        .get("refs/heads/main")
        .and_then(|v| v.as_str())
        .expect("main watermark present");
    assert_eq!(advanced, main_head_after, "watermark advanced to new tip");
}

//! AC-IDX-1: initial full walk persists every reachable commit, attaches
//! every ref, populates tags, and writes a watermark per ref
//! (M3-6, TDD-000 §2.2, SPEC-001 §5).
//!
//! Fixture: 10 commits across `main` + `feature/x` + the annotated tag
//! `v1.0` pointing at the 6th commit of `main`.

mod common;

use rusqlite::Connection;

use gitlore_core::index::indexer::{Indexer, INDEX_DB_FILENAME, WATERMARK_KEY};
use gitlore_core::index::lock::LockMode;
use gitlore_core::index::storage::resolve_index_path;

use common::Fixture;

fn build_repo(fx: &Fixture) {
    // Six commits on `main`.
    for i in 1..=6 {
        fx.commit(
            &format!("src/file_{i}.rs"),
            &format!("// version {i}"),
            &format!("feat: main commit {i}"),
        );
    }
    fx.tag("v1.0", true);

    // Two more on `main` after the tag.
    fx.commit("src/file_7.rs", "// version 7", "feat: main commit 7");
    fx.commit("src/file_8.rs", "// version 8", "feat: main commit 8");

    // Two more on `feature/x` branching from current main HEAD.
    fx.checkout_new_branch("feature/x");
    fx.commit("src/feature_a.rs", "// a", "feat: feature commit a");
    fx.commit("src/feature_b.rs", "// b", "feat: feature commit b");
}

#[test]
fn initial_full_walk_persists_every_commit_and_ref() {
    let fx = Fixture::init();
    build_repo(&fx);
    let main_head = {
        fx.checkout("main");
        fx.head()
    };
    let feature_head = {
        fx.checkout("feature/x");
        fx.head()
    };
    let v10_target = common::capture_git(&fx.repo, &["rev-parse", "v1.0^{commit}"]);

    let mut indexer = Indexer::open(&fx.repo, LockMode::NoWait).expect("open");
    let mut progress_ticks = 0u32;
    let report = indexer
        .run_initial(&mut |_processed, _total| {
            progress_ticks += 1;
        })
        .expect("run_initial");

    assert!(progress_ticks > 0, "progress callback must fire");
    assert_eq!(
        report.commits_indexed, 10,
        "10 commits across both branches"
    );
    assert_eq!(report.commits_total, 10);

    // Open a separate read-only view onto the index for verification.
    let db_path = {
        let provider = gitlore_core::git::cli::GitCliProvider::new(fx.repo.clone());
        let loc = resolve_index_path(&fx.repo, &provider).unwrap();
        loc.path().join(INDEX_DB_FILENAME)
    };
    let conn = Connection::open(db_path).unwrap();

    let commit_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(commit_count, 10, "commits table = 10 rows");

    let ref_names: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT ref_name FROM commit_refs ORDER BY ref_name")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };
    for expected in ["refs/heads/main", "refs/heads/feature/x", "refs/tags/v1.0"] {
        assert!(
            ref_names.iter().any(|n| n == expected),
            "commit_refs missing {expected}; got {ref_names:?}"
        );
    }

    let tag_sha: String = conn
        .query_row(
            "SELECT sha FROM tags WHERE ref_name = 'refs/tags/v1.0'",
            [],
            |r| r.get(0),
        )
        .expect("v1.0 tag row");
    assert_eq!(tag_sha, v10_target, "v1.0 deref to commit");

    // Every commit must have non-empty dirs_touched (`src` for all).
    let empty_dirs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM commits WHERE dirs_touched = '[]'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(empty_dirs, 0, "every commit has at least one dir");

    // Watermark JSON contains the head SHA of each ref.
    let watermark_raw: String = conn
        .query_row(
            "SELECT value FROM index_state WHERE key = ?1",
            [WATERMARK_KEY],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&watermark_raw).expect("watermark JSON");
    let obj = parsed.as_object().expect("watermark is an object");
    assert_eq!(
        obj.get("refs/heads/main").and_then(|v| v.as_str()),
        Some(main_head.as_str()),
        "main watermark = main head"
    );
    assert_eq!(
        obj.get("refs/heads/feature/x").and_then(|v| v.as_str()),
        Some(feature_head.as_str()),
        "feature/x watermark = feature/x head"
    );
    assert!(
        obj.contains_key("refs/tags/v1.0"),
        "tag watermark recorded for v1.0"
    );
}

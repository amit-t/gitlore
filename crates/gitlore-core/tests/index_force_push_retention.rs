//! AC-IDX-5: force-push retention (M3-6, TDD-000 §2.2, Q8 / ADR).
//!
//! Index a repo with `refs/heads/main` pointing at commit `C3`, then
//! force-reset `main` back to `C1` (orphaning `C2` and `C3`), expire
//! the reflog + `git gc --prune=now` so the orphans are actually
//! unreachable via `cat-file -e`, then run incremental (or
//! `prune_orphans` directly).
//!
//! Expected outcome:
//! * `commit_refs` rows for `C2` and `C3` are deleted.
//! * `commits` rows for `C2` and `C3` are retained (so the
//!   `--include-unreachable` query at M3-7 can find them).
//! * A LEFT JOIN selecting commits with no `commit_refs` row returns
//!   exactly the orphan pair.

mod common;

use rusqlite::Connection;

use gitlore_core::index::indexer::{Indexer, INDEX_DB_FILENAME};
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
fn force_push_orphans_drop_commit_refs_but_keep_commits() {
    let fx = Fixture::init();
    let c1 = fx.commit("src/a.rs", "// 1", "feat: c1");
    let c2 = fx.commit("src/b.rs", "// 2", "feat: c2");
    let c3 = fx.commit("src/c.rs", "// 3", "feat: c3");

    let initial_report = {
        let mut indexer = Indexer::open(&fx.repo, LockMode::NoWait).expect("open");
        indexer.run_initial(&mut |_, _| {}).expect("run_initial")
    };
    assert_eq!(initial_report.commits_indexed, 3);

    // Force-reset `main` back to C1 + GC so C2 / C3 are truly orphans.
    fx.reset_hard(&c1);
    fx.gc_now();

    // Confirm cat-file no longer resolves the orphans (probe with the
    // same provider the indexer uses).
    {
        let provider = gitlore_core::git::cli::GitCliProvider::new(fx.repo.clone());
        use gitlore_core::git::{GitProvider, Sha};
        assert!(
            !provider.cat_file_exists(&Sha::new(&c2).unwrap()).unwrap(),
            "C2 should be unreachable after gc"
        );
        assert!(
            !provider.cat_file_exists(&Sha::new(&c3).unwrap()).unwrap(),
            "C3 should be unreachable after gc"
        );
    }

    // Run incremental — pulls in nothing new but must prune the orphans.
    {
        let mut indexer = Indexer::open(&fx.repo, LockMode::NoWait).expect("open");
        indexer
            .run_incremental(&mut |_, _| {})
            .expect("incremental");
    }

    let conn = db_conn(&fx);

    // commits table retains every row (including the orphans).
    let commit_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(commit_count, 3, "orphan commit rows retained");

    // commit_refs rows for C2 / C3 must be gone.
    let orphan_refs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM commit_refs WHERE sha IN (?1, ?2)",
            [&c2, &c3],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphan_refs, 0, "orphan commit_refs rows pruned");

    // commit_refs for C1 still attached to main.
    let c1_refs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM commit_refs WHERE sha = ?1",
            [&c1],
            |r| r.get(0),
        )
        .unwrap();
    assert!(c1_refs >= 1, "C1 still attached to refs");

    // --include-unreachable view: commits with no commit_refs row.
    let unreachable: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT c.sha FROM commits c \
                 WHERE NOT EXISTS (SELECT 1 FROM commit_refs r WHERE r.sha = c.sha) \
                 ORDER BY c.sha",
            )
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };
    assert!(unreachable.contains(&c2), "C2 visible as orphan");
    assert!(unreachable.contains(&c3), "C3 visible as orphan");
    assert!(!unreachable.contains(&c1), "C1 still has a ref");
}

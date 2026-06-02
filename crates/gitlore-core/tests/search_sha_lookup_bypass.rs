//! AC-SEARCH-3: SHA-prefix queries bypass FTS5 and return exactly one hit
//! with mode `ShaLookup` (M4 TDD-001 §2.2, SPEC-001 §4.3.1).
//!
//! Fixture: 2 commits. We capture the full SHA of the second commit and run
//! the search with its first 10 hex characters. Expected:
//!   1. Exactly 1 result.
//!   2. The result's SHA equals the full commit SHA.
//!   3. `results.mode == SearchMode::ShaLookup`.

mod common;

use std::sync::Arc;

use gitlore_core::git::cli::GitCliProvider;
use gitlore_core::index::indexer::{Indexer, INDEX_DB_FILENAME};
use gitlore_core::index::lock::LockMode;
use gitlore_core::index::storage::resolve_index_path;
use gitlore_core::search::clock::SystemClock;
use gitlore_core::search::conn_pool::SearchConnPool;
use gitlore_core::search::orchestrator::SearchOrchestrator;
use gitlore_core::search::types::{Filters, Query, SearchMode};
use gitlore_core::SearchConfig;

use common::Fixture;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn index_repo(repo: &std::path::Path) {
    let mut idx = Indexer::open(repo, LockMode::Wait).expect("Indexer::open");
    idx.run_initial(&mut |_, _| {}).expect("run_initial");
}

fn open_orch(repo: &std::path::Path) -> SearchOrchestrator {
    let provider = GitCliProvider::new(repo.to_path_buf());
    let loc = resolve_index_path(repo, &provider).expect("resolve_index_path");
    let db_path = loc.path().join(INDEX_DB_FILENAME);
    let pool = SearchConnPool::open(&db_path).expect("SearchConnPool::open");
    let config = SearchConfig::default();
    let clock = Arc::new(SystemClock);
    SearchOrchestrator::new(pool, config, clock)
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[test]
fn sha_prefix_bypasses_fts5_and_returns_sha_lookup_mode() {
    let fx = Fixture::init();

    // First commit — noise.
    fx.commit(
        "src/lib.rs",
        "// initial lib",
        "chore: initial library scaffold",
    );

    // Second commit — the one we will look up by SHA prefix.
    let sha = fx.commit(
        "src/retry.rs",
        "// retry logic",
        "fix: add retry logic on network timeout",
    );

    // Sanity-check: the SHA returned by Fixture is a full 40-char hex string.
    assert_eq!(sha.len(), 40, "fixture SHA must be 40 chars");

    index_repo(&fx.repo);
    let orch = open_orch(&fx.repo);

    // Use the first 10 characters as the search text.
    // The SHA-bypass fires for 4-40 lowercase hex chars.
    let prefix = sha[..10].to_string();

    let q = Query {
        text: prefix.clone(),
        filters: Filters::default(),
        limit: 10,
    };
    let results = orch.query(&q).expect("query");

    // 1. Exactly 1 result (unique prefix).
    assert_eq!(
        results.results.len(),
        1,
        "expected exactly 1 result for unique SHA prefix {:?}, got {}",
        prefix,
        results.results.len()
    );

    // 2. The returned SHA equals the full commit SHA.
    assert_eq!(
        results.results[0].sha, sha,
        "result SHA mismatch: got {:?}, want {:?}",
        results.results[0].sha, sha
    );

    // 3. Mode is ShaLookup.
    assert_eq!(
        results.mode,
        SearchMode::ShaLookup,
        "expected SearchMode::ShaLookup, got {:?}",
        results.mode
    );
}

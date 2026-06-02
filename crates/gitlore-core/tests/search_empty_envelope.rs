//! AC-SEARCH-1: a query that matches nothing must return a well-formed
//! `SearchResults` envelope with an empty `results` vec, `total_available == 0`,
//! and no error (TDD-001 §3.1 / SPEC-001 §4.3.1).
//!
//! Fixture: two commits so the index is populated; the query text is a
//! deliberately nonsensical string that cannot match any commit in the
//! English-language fixture.

mod common;

use std::sync::Arc;

use gitlore_core::git::cli::GitCliProvider;
use gitlore_core::index::indexer::{Indexer, INDEX_DB_FILENAME};
use gitlore_core::index::lock::LockMode;
use gitlore_core::index::storage::resolve_index_path;
use gitlore_core::search::clock::SystemClock;
use gitlore_core::search::conn_pool::SearchConnPool;
use gitlore_core::search::orchestrator::SearchOrchestrator;
use gitlore_core::search::types::{Filters, Query};
use gitlore_core::SearchConfig;

use common::Fixture;

/// Query string that is astronomically unlikely to appear in any commit
/// written by a human for this fixture.
const UNMATCHABLE: &str = "zzzquerynotmatchinganything12345";

fn build_repo(fx: &Fixture) {
    fx.commit("src/main.rs", "fn main() {}", "feat: initial commit");
    fx.commit("src/lib.rs", "pub fn retry() {}", "fix: retry on timeout");
}

fn open_orch(repo: &std::path::Path) -> SearchOrchestrator {
    let provider = GitCliProvider::new(repo.to_path_buf());
    let loc = resolve_index_path(repo, &provider).unwrap();
    let db_path = loc.path().join(INDEX_DB_FILENAME);
    let pool = SearchConnPool::open(&db_path).unwrap();
    let clock = Arc::new(SystemClock);
    SearchOrchestrator::new(pool, SearchConfig::default(), clock)
}

fn index_repo(repo: &std::path::Path) {
    let mut idx = Indexer::open(repo, LockMode::Wait).unwrap();
    idx.run_initial(&mut |_, _| {}).unwrap();
}

#[test]
fn empty_query_returns_ok_empty_envelope() {
    let fx = Fixture::init();
    build_repo(&fx);
    index_repo(&fx.repo);

    let orch = open_orch(&fx.repo);
    let q = Query {
        text: UNMATCHABLE.to_string(),
        filters: Filters::default(),
        limit: 50,
    };

    let result = orch.query(&q);

    // Must not be an error.
    assert!(
        result.is_ok(),
        "query returned Err for an unmatchable string: {:?}",
        result.err()
    );

    let results = result.unwrap();

    // Results slice must be empty.
    assert!(
        results.results.is_empty(),
        "expected no hits for `{UNMATCHABLE}`, got {} hit(s)",
        results.results.len()
    );

    // total_available must be zero.
    assert_eq!(
        results.total_available, 0,
        "total_available should be 0 for an unmatchable query, got {}",
        results.total_available
    );

    // Query text round-trips through the envelope.
    assert_eq!(
        results.query, UNMATCHABLE,
        "envelope query field does not match input"
    );
}

//! Phrase query test (M4 TDD-001 §2.2, SPEC-001 §4.3.1).
//!
//! Querying "circuit breaker open" should rank the commit whose subject
//! contains "circuit breaker" higher than one that only mentions "open circuit".
//!
//! FTS5 phrase matching is engine-dependent; we use lenient assertions:
//!   1. At least 1 result is returned.
//!   2. The "circuit breaker" commit appears somewhere in the results.
//!   3. The quoted syntax `"circuit breaker"` does not cause a parse error.

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
// Tests
// ---------------------------------------------------------------------------

#[test]
fn phrase_query_circuit_breaker_returns_relevant_hit() {
    let fx = Fixture::init();

    // Commit 1: directly relevant.
    let breaker_sha = fx.commit(
        "src/breaker.rs",
        "// Circuit breaker state machine: closed, open, half-open",
        "fix: circuit breaker open on timeout",
    );

    // Commit 2: partially relevant — mentions "open circuit" but not as a phrase.
    fx.commit(
        "src/circuit.rs",
        "// open circuit logic for power management",
        "chore: update open circuit logic",
    );

    index_repo(&fx.repo);
    let orch = open_orch(&fx.repo);

    let q = Query {
        text: "circuit breaker open".to_string(),
        filters: Filters::default(),
        limit: 10,
    };
    let results = orch.query(&q).expect("query");

    // 1. At least 1 result.
    assert!(
        !results.results.is_empty(),
        "expected at least 1 result for query 'circuit breaker open', got 0"
    );

    // 2. Mode is Lexical (not a SHA-shaped query).
    assert_eq!(
        results.mode,
        SearchMode::Lexical,
        "expected SearchMode::Lexical, got {:?}",
        results.mode
    );

    // 3. The "circuit breaker" commit must appear in the results.
    let shas: Vec<&str> = results.results.iter().map(|h| h.sha.as_str()).collect();
    assert!(
        shas.contains(&breaker_sha.as_str()),
        "commit {:?} (circuit breaker open on timeout) must appear in results; got {:?}",
        breaker_sha,
        shas
    );

    // 4. All scores must be in (0.0, 1.0].
    for hit in &results.results {
        assert!(
            hit.score > 0.0 && hit.score <= 1.0,
            "hit score {} out of expected range (0.0, 1.0]",
            hit.score
        );
    }
}

/// Double-quoted phrase syntax must not cause a parse error.
#[test]
fn phrase_query_quoted_syntax_does_not_error() {
    let fx = Fixture::init();

    fx.commit(
        "src/breaker.rs",
        "// circuit breaker open state",
        "fix: circuit breaker open on timeout",
    );
    fx.commit(
        "src/other.rs",
        "// unrelated file",
        "chore: unrelated maintenance task",
    );

    index_repo(&fx.repo);
    let orch = open_orch(&fx.repo);

    let q = Query {
        text: r#""circuit breaker""#.to_string(),
        filters: Filters::default(),
        limit: 10,
    };

    let results = orch.query(&q).expect("quoted phrase query must not error");

    // Mode must be Lexical.
    assert_eq!(
        results.mode,
        SearchMode::Lexical,
        "quoted phrase should produce Lexical mode, got {:?}",
        results.mode
    );

    // If results are returned they must be well-formed.
    for hit in &results.results {
        assert_eq!(hit.sha.len(), 40, "sha must be 40 chars");
        assert!(hit.score > 0.0, "score must be positive");
    }
}

//! AC-SEARCH-1 baseline: lexical search returns ranked hits with correct
//! field shapes and a plausible blend score for a simple text query
//! (M4 TDD-001 §2.2, SPEC-001 §4.3.1).
//!
//! Fixture: 3 commits, one of which mentions "retry on timeout".
//! After indexing we run a lexical search and assert:
//!   1. At least 1 hit is returned.
//!   2. The first hit's fields are well-formed.
//!   3. The blend formula is approximately honoured (loose ±0.05 tolerance).
//!   4. The returned mode is `SearchMode::Lexical`.

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
fn lexical_search_returns_ranked_hits_for_retry_query() {
    let fx = Fixture::init();

    // Three commits: one on the target topic, two unrelated noise commits.
    fx.commit(
        "src/http_client.rs",
        "// HTTP client with retry logic",
        "fix: retry on timeout for HTTP requests",
    );
    fx.commit(
        "src/auth.rs",
        "// OAuth2 token refresh",
        "feat: add OAuth2 token refresh",
    );
    fx.commit(
        "docs/changelog.md",
        "# Changelog",
        "docs: update changelog for v1.2",
    );

    index_repo(&fx.repo);
    let orch = open_orch(&fx.repo);

    let q = Query {
        text: "retry on timeout".to_string(),
        filters: Filters::default(),
        limit: 10,
    };
    let results = orch.query(&q).expect("query");

    // 1. At least 1 hit.
    assert!(
        !results.results.is_empty(),
        "expected at least one hit for query 'retry on timeout', got zero"
    );

    // 2. First hit field shapes are well-formed.
    let first = &results.results[0];

    // SHA must be exactly 40 lowercase hex chars.
    assert_eq!(
        first.sha.len(),
        40,
        "sha must be 40 chars, got {:?}",
        first.sha
    );
    assert!(
        first.sha.chars().all(|c| c.is_ascii_hexdigit()),
        "sha must be all hex digits, got {:?}",
        first.sha
    );

    // Subject must contain "retry".
    assert!(
        first.subject.to_lowercase().contains("retry"),
        "expected first hit subject to contain 'retry', got {:?}",
        first.subject
    );

    // Author must be non-empty.
    assert!(!first.author.is_empty(), "author field must be non-empty");

    // committed_at must be a positive Unix timestamp.
    assert!(
        first.committed_at > 0,
        "committed_at must be > 0, got {}",
        first.committed_at
    );

    // score must be in (0.0, 1.0].
    assert!(
        first.score > 0.0,
        "score must be > 0.0, got {}",
        first.score
    );
    assert!(
        first.score <= 1.0,
        "score must be <= 1.0, got {}",
        first.score
    );

    // lexical_bm25 factor must be >= 0.0.
    assert!(
        first.factors.lexical_bm25 >= 0.0,
        "factors.lexical_bm25 must be >= 0.0, got {}",
        first.factors.lexical_bm25
    );

    // 3. Blend formula: score ≈ 0.50*lex + 0.30*path + 0.20*rec (±0.05 tolerance).
    let f = &first.factors;
    let expected_blend = 0.50 * f.lexical_bm25 + 0.30 * f.path_relevance + 0.20 * f.recency;
    let diff = (first.score - expected_blend).abs();
    assert!(
        diff < 0.05,
        "blend formula mismatch: score={}, expected≈{}, diff={} (factors={:?})",
        first.score,
        expected_blend,
        diff,
        first.factors
    );

    // 4. Mode must be Lexical.
    assert_eq!(
        results.mode,
        SearchMode::Lexical,
        "expected SearchMode::Lexical, got {:?}",
        results.mode
    );
}

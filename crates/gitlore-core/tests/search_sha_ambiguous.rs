//! Tests for the SHA-lookup path: happy-path single resolution and
//! compile-time verification that `Error::ShaAmbiguousPrefix` is constructible
//! and maps to the stable wire code `"sha_ambiguous_prefix"`.
//!
//! The truly ambiguous case (two commits sharing an 8-char prefix) is not
//! deterministically constructible via the fixture API since SHA values are
//! determined by git; we document this and cover the error type's API with a
//! compile-only assertion.
//!
//! Integration tests for the happy path live in `search_sha_lookup_bypass.rs`.

mod common;

use std::sync::Arc;

use gitlore_core::error::Error;
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

/// Happy path: a unique SHA prefix resolves to exactly one commit in
/// `ShaLookup` mode (single-commit fixture edge case).
#[test]
fn sha_lookup_unique_prefix_resolves_to_sha_lookup_mode() {
    let fx = Fixture::init();

    let sha = fx.commit("src/main.rs", "fn main() {}", "chore: initial commit");
    assert_eq!(sha.len(), 40, "SHA must be 40 chars");

    index_repo(&fx.repo);
    let orch = open_orch(&fx.repo);

    // 8-char prefix — well above the 4-char minimum for SHA detection.
    let prefix = sha[..8].to_string();
    let q = Query {
        text: prefix.clone(),
        filters: Filters::default(),
        limit: 10,
    };
    let results = orch.query(&q).expect("query");

    assert_eq!(
        results.mode,
        SearchMode::ShaLookup,
        "single-commit repo: expected ShaLookup mode for prefix {prefix:?}",
    );
    assert_eq!(
        results.results.len(),
        1,
        "single-commit repo: expected 1 result, got {}",
        results.results.len()
    );
    assert_eq!(
        results.results[0].sha, sha,
        "resolved SHA must match the full commit SHA"
    );
}

/// Compile-time + runtime verification that `Error::ShaAmbiguousPrefix` is
/// constructible and returns the correct stable wire code.
#[test]
fn sha_ambiguous_prefix_error_type_is_correct() {
    let err = Error::ShaAmbiguousPrefix {
        prefix: "dead1234".to_string(),
        count: 2,
    };

    // Stable wire code per SPEC-001 §4.3.
    let code = err.code();
    assert_eq!(
        code, "sha_ambiguous_prefix",
        "stable error code drifted; got {code:?}",
    );

    // Display must mention the prefix.
    let rendered = format!("{err}");
    assert!(
        !rendered.is_empty(),
        "Display impl must produce a non-empty message"
    );
    assert!(
        rendered.contains("dead1234"),
        "Display message should mention the prefix, got: {rendered:?}",
    );
}

/// When a SHA-shaped query does not match any indexed commit the orchestrator
/// must return `Ok(...)` (not an error).
#[test]
fn sha_lookup_no_match_returns_ok() {
    let fx = Fixture::init();
    fx.commit("README.md", "# readme", "docs: add readme");

    index_repo(&fx.repo);
    let orch = open_orch(&fx.repo);

    // All-zeros is extremely unlikely to collide with any real commit SHA.
    let q = Query {
        text: "00000000".to_string(),
        filters: Filters::default(),
        limit: 10,
    };
    // Must not panic or return Err.
    let result = orch.query(&q);
    assert!(
        result.is_ok(),
        "non-matching SHA prefix must not return Err, got {:?}",
        result.err()
    );
}

//! `search.api-nodejs` — 30-query private fixture scenario (M4 / AC-SEARCH-2).
//!
//! This scenario is gated on `GITLORE_EVAL_FIXTURES_PRIVATE=1` AND the
//! presence of `qa/fixtures-private/api-nodejs/`. When either gate fails it
//! emits `passed: true` with a `skipped:` summary.
//!
//! Acceptance criterion: top-5 precision >= 0.80 across the 30 labelled
//! queries (median-of-3 per query to smooth index-ordering variance).

use std::io;
use std::path::Path;

use gitlore_core::index::indexer::Indexer;
use gitlore_core::index::lock::LockMode;
use gitlore_core::search::clock::SystemClock;
use gitlore_core::search::conn_pool::SearchConnPool;
use gitlore_core::search::orchestrator::SearchOrchestrator;
use gitlore_core::search::types::{Filters, Query};
use gitlore_core::SearchConfig;

use crate::fixtures::{load_private, workspace_root};
use crate::metrics::search_top_k_precision;
use crate::scenarios::{Scenario, ScenarioReport};

/// Stable scenario name.
pub const NAME: &str = "search.api-nodejs";

/// Relative path to the private fixture repo from workspace root.
pub const FIXTURE_REL: &str = "qa/fixtures-private/api-nodejs";

/// Top-5 precision threshold (AC-SEARCH-2 for private lane: >= 0.80).
pub const TOP_K: usize = 5;
/// Minimum acceptable top-5 precision for the api-nodejs private fixture.
pub const PRECISION_THRESHOLD: f32 = 0.80;

/// Number of repeated query runs per query (median-of-3 smoothing).
pub const RUNS_PER_QUERY: usize = 3;

/// Hard-coded query labels for the api-nodejs fixture.
/// Format: (query_text, expected_sha_substring). The expected_sha_substring is
/// matched as a prefix against indexed SHAs. These must be filled in by the
/// fixture maintainer once the private repo is indexed (Amit provides labels).
const LABELLED_QUERIES: &[(&str, &str)] = &[
    // Placeholder labels — to be replaced by fixture maintainer.
    ("fix retry logic", ""),
    ("add circuit breaker", ""),
    ("update express middleware", ""),
    ("refactor error handling", ""),
    ("add unit tests for router", ""),
    ("bump dependencies", ""),
    ("performance tuning cache", ""),
    ("fix memory leak", ""),
    ("add logging middleware", ""),
    ("database connection pool", ""),
    ("config from environment", ""),
    ("migration version bump", ""),
    ("fix race condition", ""),
    ("add health check endpoint", ""),
    ("refactor auth token", ""),
    ("remove deprecated api", ""),
    ("fix cors configuration", ""),
    ("add integration tests", ""),
    ("update readme docs", ""),
    ("fix null pointer exception", ""),
    ("add request validation", ""),
    ("refactor service layer", ""),
    ("improve error messages", ""),
    ("add rate limiting", ""),
    ("fix timeout handling", ""),
    ("update type definitions", ""),
    ("add prometheus metrics", ""),
    ("fix serialization error", ""),
    ("add graceful shutdown", ""),
    ("update openapi schema", ""),
];

/// Scenario runner for the 30-query private api-nodejs fixture.
pub struct SearchApiNodejs;

impl Scenario for SearchApiNodejs {
    fn name(&self) -> &'static str {
        NAME
    }

    fn run(&self) -> ScenarioReport {
        let fixtures = load_private();
        if !fixtures.is_available() {
            return ScenarioReport::new(NAME)
                .with_summary(format!(
                    "skipped: {} (lights up on self-hosted lane with GITLORE_EVAL_FIXTURES_PRIVATE=1)",
                    fixtures.skip_reason.as_deref().unwrap_or("private fixture absent")
                ))
                .passed();
        }

        let fixture_dir = workspace_root().join(FIXTURE_REL);
        if !Path::new(&fixture_dir).is_dir() {
            return ScenarioReport::new(NAME)
                .with_summary(format!(
                    "skipped: private fixture {FIXTURE_REL}/ not present"
                ))
                .passed();
        }

        match run_api_nodejs(&fixture_dir) {
            Ok(report) => report,
            Err(e) => ScenarioReport::new(NAME).with_summary(format!("failed: {e}")),
        }
    }
}

fn run_api_nodejs(fixture_dir: &Path) -> io::Result<ScenarioReport> {
    // Open the indexer against the private fixture (already indexed; we open
    // read-only for search).
    let provider = gitlore_core::git::cli::GitCliProvider::new(fixture_dir.to_path_buf());
    let index_location = gitlore_core::index::storage::resolve_index_path(fixture_dir, &provider)
        .map_err(|e| io::Error::other(format!("resolve_index_path: {e}")))?;

    // Run initial index if not yet indexed.
    if !index_location.path().exists() {
        let mut indexer = Indexer::open(fixture_dir, LockMode::Wait)
            .map_err(|e| io::Error::other(format!("indexer open: {e}")))?;
        indexer
            .run_initial(&mut |_, _| {})
            .map_err(|e| io::Error::other(format!("run_initial: {e}")))?;
    }

    let pool = SearchConnPool::open(index_location.path())
        .map_err(|e| io::Error::other(format!("pool open: {e}")))?;
    let config = SearchConfig::default();
    let clock = std::sync::Arc::new(SystemClock);
    let orch = SearchOrchestrator::new(pool, config, clock);

    let mut expected_shas: Vec<String> = Vec::new();
    let mut result_sha_lists: Vec<Vec<String>> = Vec::new();

    for (query_text, expected_sha_prefix) in LABELLED_QUERIES {
        // Median-of-3: run each query 3 times and take the result set from
        // the middle run (all three should be deterministic for lexical search,
        // but we follow the spec).
        let mut run_results: Vec<Vec<String>> = Vec::new();
        for _ in 0..RUNS_PER_QUERY {
            let q = Query {
                text: query_text.to_string(),
                filters: Filters::default(),
                limit: TOP_K as u32,
            };
            let result = orch
                .query(&q)
                .map_err(|e| io::Error::other(format!("query: {e}")))?;
            run_results.push(result.results.iter().map(|h| h.sha.clone()).collect());
        }
        // Median-of-3 = middle run for deterministic lexical (all equal).
        let shas = run_results
            .into_iter()
            .nth(RUNS_PER_QUERY / 2)
            .unwrap_or_default();
        expected_shas.push(expected_sha_prefix.to_string());
        result_sha_lists.push(shas);
    }

    // Filter out queries with empty expected SHAs (unlabelled).
    let labelled: Vec<_> = expected_shas
        .iter()
        .zip(result_sha_lists.iter())
        .filter(|(exp, _)| !exp.is_empty())
        .collect();

    if labelled.is_empty() {
        return Ok(ScenarioReport::new(NAME)
            .with_summary(
                "skipped: no labelled queries in private fixture (fix_plan labels pending)",
            )
            .passed());
    }

    let expected_refs: Vec<&str> = labelled.iter().map(|(e, _)| e.as_str()).collect();
    let result_refs: Vec<Vec<&str>> = labelled
        .iter()
        .map(|(_, v)| v.iter().map(String::as_str).collect())
        .collect();
    let result_ref_slices: Vec<&[&str]> = result_refs.iter().map(|v| v.as_slice()).collect();

    let precision = search_top_k_precision(&expected_refs, &result_ref_slices, TOP_K);
    let passed = precision >= PRECISION_THRESHOLD;

    Ok(ScenarioReport::new(NAME)
        .with_metric("top5_precision", precision as f64)
        .with_metric("labelled_queries", labelled.len() as f64)
        .with_summary(format!(
            "{}: top-5 precision = {precision:.3} over {} labelled queries (threshold = {PRECISION_THRESHOLD:.2})",
            if passed { "ok" } else { "FAIL" },
            labelled.len(),
        ))
        .passed_if(passed))
}

trait ScenarioReportExt {
    fn passed_if(self, cond: bool) -> Self;
}

impl ScenarioReportExt for ScenarioReport {
    fn passed_if(mut self, cond: bool) -> Self {
        self.passed = cond;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_stable() {
        assert_eq!(SearchApiNodejs.name(), "search.api-nodejs");
    }

    #[test]
    fn skips_when_private_fixture_absent() {
        // Without GITLORE_EVAL_FIXTURES_PRIVATE=1, the scenario must skip-pass.
        let was_set = std::env::var(crate::fixtures::ENV_PRIVATE).ok();
        std::env::remove_var(crate::fixtures::ENV_PRIVATE);
        let report = SearchApiNodejs.run();
        if let Some(v) = was_set {
            std::env::set_var(crate::fixtures::ENV_PRIVATE, v);
        }
        assert_eq!(report.scenario, NAME);
        assert!(report.passed);
        assert!(report.summary.starts_with("skipped:"));
    }
}

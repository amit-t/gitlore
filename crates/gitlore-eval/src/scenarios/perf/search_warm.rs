//! `perf.search_warm` — warm search latency gate (M4 / TDD-001 / PRD-001 §5.1).
//!
//! Pre-warms the connection by running one throwaway query, then executes 3
//! timed iterations of `gitlore search "retry"` via the
//! `SearchOrchestrator` API. Reports p95 and p99 from the 3-sample set.
//!
//! ## Acceptance criteria (PRD-001 §5.1)
//!
//! * p95 < 100 ms
//! * p99 < 250 ms
//!
//! ## Lane behaviour
//!
//! This scenario requires the `api-nodejs` private fixture (already indexed)
//! to run against a realistic data volume. Without it the scenario emits
//! `passed: true` with a `skipped:` summary — the gate is dormant on the
//! public hosted lane.

use std::io;
use std::path::Path;
use std::time::Instant;

use gitlore_core::index::indexer::INDEX_DB_FILENAME;
use gitlore_core::search::clock::SystemClock;
use gitlore_core::search::conn_pool::SearchConnPool;
use gitlore_core::search::orchestrator::SearchOrchestrator;
use gitlore_core::search::types::{Filters, Query};
use gitlore_core::SearchConfig;

use crate::fixtures::workspace_root;
use crate::metrics::percentile;
use crate::scenarios::{Scenario, ScenarioReport};

/// Stable scenario name.
pub const NAME: &str = "perf.search_warm";

/// Private fixture path (same repo as the cold-index scenario).
pub const FIXTURE_REL: &str = "qa/fixtures-private/api-nodejs";

/// Number of timed iterations after the warm-up query.
pub const TIMED_RUNS: usize = 3;

/// p95 budget in milliseconds (PRD-001 §5.1).
pub const P95_BUDGET_MS: u64 = 100;

/// p99 budget in milliseconds (PRD-001 §5.1).
pub const P99_BUDGET_MS: u64 = 250;

/// Scenario runner for the warm search latency perf gate.
pub struct SearchWarm;

impl Scenario for SearchWarm {
    fn name(&self) -> &'static str {
        NAME
    }

    fn run(&self) -> ScenarioReport {
        let fixture_dir = workspace_root().join(FIXTURE_REL);
        if !Path::new(&fixture_dir).is_dir() {
            return ScenarioReport::new(NAME)
                .with_summary(format!(
                    "skipped: private fixture {FIXTURE_REL}/ not present \
                     (lights up on self-hosted lane)"
                ))
                .passed();
        }

        match run_warm_search(&fixture_dir) {
            Ok(report) => report,
            Err(e) => ScenarioReport::new(NAME).with_summary(format!("failed: {e}")),
        }
    }
}

fn run_warm_search(fixture_dir: &Path) -> io::Result<ScenarioReport> {
    let provider = gitlore_core::git::cli::GitCliProvider::new(fixture_dir.to_path_buf());
    let index_location = gitlore_core::index::storage::resolve_index_path(fixture_dir, &provider)
        .map_err(|e| io::Error::other(format!("resolve_index_path: {e}")))?;

    let pool = SearchConnPool::open(&index_location.path().join(INDEX_DB_FILENAME))
        .map_err(|e| io::Error::other(format!("pool open: {e}")))?;
    let config = SearchConfig::default();
    let clock = std::sync::Arc::new(SystemClock);
    let orch = SearchOrchestrator::new(pool, config, clock);

    let warm_query = Query {
        text: "retry".to_string(),
        filters: Filters::default(),
        limit: 50,
    };

    // Warm-up: one throwaway query.
    let _ = orch
        .query(&warm_query)
        .map_err(|e| io::Error::other(format!("warm-up query: {e}")))?;

    // Timed runs.
    let mut samples_us: Vec<u128> = Vec::with_capacity(TIMED_RUNS);
    for _ in 0..TIMED_RUNS {
        let start = Instant::now();
        let _ = orch
            .query(&warm_query)
            .map_err(|e| io::Error::other(format!("timed query: {e}")))?;
        samples_us.push(start.elapsed().as_micros());
    }

    // Convert to ms for reporting.
    let samples_ms: Vec<u128> = samples_us.iter().map(|us| us / 1000).collect();
    let p95_ms = percentile(&samples_ms, 95) as u64;
    let p99_ms = percentile(&samples_ms, 99) as u64;
    let passed = p95_ms < P95_BUDGET_MS && p99_ms < P99_BUDGET_MS;

    Ok(ScenarioReport::new(NAME)
        .with_metric("p95_ms", p95_ms as f64)
        .with_metric("p99_ms", p99_ms as f64)
        .with_metric("samples", TIMED_RUNS as f64)
        .with_summary(format!(
            "{}: p95={p95_ms}ms p99={p99_ms}ms over {TIMED_RUNS} warm runs \
             (budgets: p95<{P95_BUDGET_MS}ms p99<{P99_BUDGET_MS}ms)",
            if passed { "ok" } else { "FAIL" }
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
        assert_eq!(SearchWarm.name(), "perf.search_warm");
    }

    #[test]
    fn skips_when_private_fixture_absent() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(crate::fixtures::ENV_WORKSPACE_ROOT, tmp.path().as_os_str());
        let report = SearchWarm.run();
        std::env::remove_var(crate::fixtures::ENV_WORKSPACE_ROOT);

        assert_eq!(report.scenario, NAME);
        assert!(report.passed);
        assert!(report.summary.starts_with("skipped:"));
    }
}

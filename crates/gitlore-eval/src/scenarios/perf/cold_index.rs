//! `perf.cold_index_api_nodejs` — SPEC-001 §7.3 cold-index gate.
//!
//! Times three cold runs of [`gitlore_core::index::indexer::Indexer::run_initial`]
//! against the `api-nodejs` fixture under the **private** fixture root and
//! asserts the p95 wall-clock stays below the 120 000 ms budget.
//!
//! ## Lane behaviour
//!
//! * **Hosted lane (public CI).** The private fixture is absent. The
//!   scenario emits `passed: true` with a `skipped: ...` summary and no
//!   metrics. The gate is dormant.
//! * **Self-hosted lane.** The private fixture is present. The scenario
//!   runs three cold iterations: copy the fixture into a fresh tempdir,
//!   open the indexer in `LockMode::Wait`, drive `run_initial`, time the
//!   wall-clock, drop the indexer + tempdir, repeat. The p95 (= max for
//!   n=3) is compared against the budget.
//!
//! ## Metrics
//!
//! Emitted on the active path only (skipped runs emit no metrics):
//!
//! | Key               | Meaning                                              |
//! |-------------------|------------------------------------------------------|
//! | `p95_ms`          | p95 of the three cold-run wall-clocks (ms). n=3 ⇒ max. |
//! | `samples`         | Number of cold runs taken (3).                       |
//! | `commits_indexed` | Commits persisted on the last run (sanity signal).   |
//!
//! ## Wall-clock scope
//!
//! Each timed window covers `Indexer::open` + `run_initial` — both costs
//! a fresh `gitlore index` invocation pays. Fixture copy + tempdir setup
//! are deliberately excluded; they are filesystem fixturing, not work the
//! production indexer does.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gitlore_core::index::indexer::Indexer;
use gitlore_core::index::lock::LockMode;

use crate::fixtures::workspace_root;
use crate::scenarios::{Scenario, ScenarioReport};

/// Stable scenario name as it appears in the registry + `gitlore-eval --list`.
pub const NAME: &str = "perf.cold_index_api_nodejs";

/// Private-fixture path checked at the workspace root.
pub const FIXTURE_REL: &str = "qa/fixtures-private/api-nodejs";

/// Number of cold iterations to run when the fixture is present.
pub const COLD_RUNS: usize = 3;

/// p95 wall-clock budget for a cold initial index walk, in milliseconds
/// (SPEC-001 §7.3).
pub const P95_BUDGET_MS: u64 = 120_000;

/// Cold-index perf scenario for the `api-nodejs` fixture.
pub struct ColdIndexApiNodejs;

impl Scenario for ColdIndexApiNodejs {
    fn name(&self) -> &'static str {
        NAME
    }

    fn run(&self) -> ScenarioReport {
        let fixture = workspace_root().join(FIXTURE_REL);
        if !Path::new(&fixture).is_dir() {
            return ScenarioReport::new(NAME)
                .with_summary(format!(
                    "skipped: private fixture {FIXTURE_REL}/ not present \
                     (lights up on self-hosted lane)"
                ))
                .passed();
        }

        let mut samples_ms: Vec<u64> = Vec::with_capacity(COLD_RUNS);
        let mut last_commits: u64 = 0;
        for run_idx in 0..COLD_RUNS {
            match cold_run(&fixture) {
                Ok((ms, commits)) => {
                    samples_ms.push(ms);
                    last_commits = commits;
                }
                Err(e) => {
                    return ScenarioReport::new(NAME).with_summary(format!(
                        "failed: cold run {n}/{COLD_RUNS} errored: {e}",
                        n = run_idx + 1
                    ));
                }
            }
        }

        // p95 over n=3 reduces to max.
        let p95_ms = samples_ms.iter().copied().max().unwrap_or(0);
        let passed = p95_ms < P95_BUDGET_MS;

        let mut metrics: BTreeMap<String, f64> = BTreeMap::new();
        metrics.insert("p95_ms".into(), p95_ms as f64);
        metrics.insert("samples".into(), COLD_RUNS as f64);
        metrics.insert("commits_indexed".into(), last_commits as f64);

        let summary = if passed {
            format!(
                "ok: p95 {p95_ms} ms over {COLD_RUNS} cold runs (< {P95_BUDGET_MS} ms budget); \
                 commits_indexed = {last_commits}"
            )
        } else {
            format!(
                "FAIL: p95 {p95_ms} ms over {COLD_RUNS} cold runs (>= {P95_BUDGET_MS} ms budget); \
                 commits_indexed = {last_commits}"
            )
        };

        ScenarioReport {
            scenario: NAME.to_string(),
            passed,
            metrics,
            summary,
        }
    }
}

/// Single cold iteration: copy fixture into a fresh tempdir, open the
/// indexer + `run_initial`, time the wall-clock, drop everything.
///
/// Returns `(wall_clock_ms, commits_indexed)` on success.
fn cold_run(fixture: &Path) -> io::Result<(u64, u64)> {
    let tmp = tempfile::tempdir()?;
    let dest: PathBuf = tmp.path().join("repo");
    copy_dir_recursive(fixture, &dest)?;

    let start = Instant::now();
    let mut indexer = Indexer::open(&dest, LockMode::Wait)
        .map_err(|e| io::Error::other(format!("indexer open: {e}")))?;
    let report = indexer
        .run_initial(&mut |_processed, _total| {})
        .map_err(|e| io::Error::other(format!("run_initial: {e}")))?;
    let elapsed_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

    drop(indexer);
    drop(tmp);
    Ok((elapsed_ms, report.commits_indexed))
}

/// Recursive directory copy. Symlinks are followed (we resolve via
/// `fs::metadata`); a symlinked subdirectory is copied as a real subdir
/// in the destination, a symlinked file is copied as a real file.
fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let md = fs::metadata(&src_path)?;
        if md.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_stable() {
        assert_eq!(ColdIndexApiNodejs.name(), "perf.cold_index_api_nodejs");
    }

    #[test]
    fn skips_passing_when_private_fixture_absent() {
        // The test runs against the live workspace. The hosted-lane
        // expectation is that `qa/fixtures-private/api-nodejs/` is absent;
        // when a self-hosted machine has it present, this assertion is
        // intentionally weakened to "report is for the right scenario".
        let r = ColdIndexApiNodejs.run();
        assert_eq!(r.scenario, "perf.cold_index_api_nodejs");
        let fixture_present = workspace_root().join(FIXTURE_REL).is_dir();
        if !fixture_present {
            assert!(r.passed, "must skip-pass on hosted lane");
            assert!(
                r.summary.starts_with("skipped:"),
                "skip summary expected, got {:?}",
                r.summary
            );
            assert!(
                r.metrics.is_empty(),
                "skip path must emit no metrics, got {:?}",
                r.metrics
            );
        }
    }

    #[test]
    fn copy_dir_recursive_mirrors_layout() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("a/b")).unwrap();
        fs::write(src.path().join("a/b/file.txt"), b"hello").unwrap();
        fs::write(src.path().join("top.txt"), b"world").unwrap();

        let dest = dst.path().join("mirror");
        copy_dir_recursive(src.path(), &dest).unwrap();

        assert_eq!(fs::read(dest.join("top.txt")).unwrap(), b"world");
        assert_eq!(fs::read(dest.join("a/b/file.txt")).unwrap(), b"hello");
    }
}

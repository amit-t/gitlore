//! `gitlore-eval` CLI binary (M2 scaffold).
//!
//! Surface:
//!
//! * `gitlore-eval --list`
//!   Print the registered scenario names, one per line. With `--json`,
//!   emits `{"scenarios": ["...", ...]}`.
//! * `gitlore-eval scenarios <name>`
//!   Load [`Registry::with_builtin_scenarios`], look up `<name>`, run it,
//!   print a [`ScenarioReport`]. Exits 0 when `passed: true`, 1 otherwise.
//!   With `--json`, emits a single-line envelope:
//!   `{"scenario":"...","passed":bool,"metrics":{...},"summary":"..."}`.
//! * `gitlore-eval --regression --baseline=<git-ref>`
//!   M4+ self-hosted lane (ADR-028 / SPEC-001 §20). At M2 every registered
//!   scenario is a stub, so this walk reports `passed: true` without
//!   executing real evaluation work — exercising the CI wiring without
//!   turning the eval-regression job red. The aggregate flips to actual
//!   scenario-by-scenario passes once concrete impls ship.
//!
//! Exit codes:
//!
//! | Outcome                                | Code |
//! |----------------------------------------|------|
//! | Scenario passed / `--list` / regression| 0    |
//! | Scenario failed or unknown name        | 1    |
//! | Argument parse / usage error           | 2    |
//!
//! Output destinations follow the project-wide convention used by `gitlore`
//! itself (`crates/gitlore/src/cli.rs`):
//! * Stdout — scenario reports, `--list`, regression envelope, and the JSON
//!   error envelope (so callers can pipe straight into `jq` with stderr
//!   muted).
//! * Stderr — human-readable error lines and clap parse failures.

use std::io::{self, Write};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;

use gitlore_eval::scenarios::{default_registry, Registry, ScenarioReport};

/// Stable string the CLI prints when `--regression` is invoked at M2.
///
/// Pulled into a `const` so the `--json` envelope and the human path agree
/// on wording (and so downstream tooling can grep for the marker if it
/// needs to distinguish a real regression run from this stub).
const REGRESSION_M2_SUMMARY: &str = "M2 stub: scenarios registered but not yet run";

#[derive(Debug, Parser)]
#[command(
    name = "gitlore-eval",
    version,
    about = "Internal evaluation harness for gitlore (search / story / risk / perf).",
    long_about = None,
)]
struct Cli {
    /// Print every registered scenario name and exit. Mutually exclusive with
    /// `--regression`. If combined with the `scenarios` subcommand, `--list`
    /// wins (checked first in `main`); the subcommand is then ignored.
    #[arg(long, conflicts_with = "regression")]
    list: bool,

    /// Run the self-hosted eval-regression lane against `--baseline`. At M2
    /// this is a stub walk: every scenario is a passing stub, so the lane
    /// reports green without doing any real evaluation work.
    #[arg(long, requires = "baseline")]
    regression: bool,

    /// Baseline git ref the regression lane should diff against. Required
    /// whenever `--regression` is set.
    #[arg(long, value_name = "GIT_REF", requires = "regression")]
    baseline: Option<String>,

    /// Run a single scenario by name (alternative to the `scenarios` subcommand).
    /// Useful for CI steps: `gitlore-eval --scenario search.synthetic`.
    #[arg(
        long,
        value_name = "SCENARIO_NAME",
        conflicts_with = "regression",
        conflicts_with = "list"
    )]
    scenario: Option<String>,

    /// Emit machine-readable JSON instead of human-readable text. Available
    /// on every mode (list / scenario run / regression walk / error).
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a single registered scenario by name.
    Scenarios {
        /// Dotted scenario name, e.g. `search.api-nodejs` or `perf.search_warm`.
        /// See `gitlore-eval --list` for the full set.
        name: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let registry = default_registry();

    if cli.list {
        return print_list(&registry, cli.json);
    }

    if cli.regression {
        // `requires = "baseline"` above means clap rejects `--regression`
        // without `--baseline` before we ever land here. The expect is a
        // defensive assertion, not a real failure mode.
        let baseline = cli
            .baseline
            .as_deref()
            .expect("clap `requires = \"baseline\"` should enforce presence");
        return run_regression(&registry, baseline, cli.json);
    }

    // `--scenario <NAME>` short-form (used by CI steps and smoke tests).
    if let Some(name) = &cli.scenario {
        return run_one(&registry, name, cli.json);
    }

    match cli.command {
        Some(Command::Scenarios { name }) => run_one(&registry, &name, cli.json),
        None => {
            // No mode chosen. Print a short hint to stderr; the JSON envelope
            // is not appropriate here because there is no semantic error
            // payload to encode — this is a usage problem.
            let _ = writeln!(
                io::stderr().lock(),
                "error: no command given. Try --help, --list, --scenario <NAME>, or `scenarios <NAME>`."
            );
            ExitCode::from(2)
        }
    }
}

/// `--list` mode.
fn print_list(registry: &Registry, json: bool) -> ExitCode {
    let mut out = io::stdout().lock();
    if json {
        let names: Vec<&str> = registry.names().collect();
        let _ = writeln!(out, "{}", json!({ "scenarios": names }));
    } else {
        for name in registry.names() {
            let _ = writeln!(out, "{name}");
        }
    }
    ExitCode::SUCCESS
}

/// `scenarios <name>` mode.
fn run_one(registry: &Registry, name: &str, json: bool) -> ExitCode {
    let Some(scenario) = registry.get(name) else {
        emit_unknown_scenario(name, json);
        return ExitCode::from(1);
    };
    let report = scenario.run();
    let passed = report.passed;
    emit_report(&report, json);
    if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `--regression --baseline=<ref>` mode.
fn run_regression(registry: &Registry, baseline: &str, json: bool) -> ExitCode {
    let names: Vec<&str> = registry.names().collect();
    let mut out = io::stdout().lock();
    if json {
        let _ = writeln!(
            out,
            "{}",
            json!({
                "regression": {
                    "baseline": baseline,
                    "passed": true,
                    "summary": REGRESSION_M2_SUMMARY,
                    "scenarios": names,
                }
            })
        );
    } else {
        let _ = writeln!(out, "regression: baseline={baseline}");
        let _ = writeln!(out, "summary:    {REGRESSION_M2_SUMMARY}");
        let _ = writeln!(out, "registered: {}", names.len());
        for name in &names {
            let _ = writeln!(out, "  - {name}");
        }
        let _ = writeln!(out, "passed:     true");
    }
    ExitCode::SUCCESS
}

/// Pretty-print a single [`ScenarioReport`].
fn emit_report(report: &ScenarioReport, json: bool) {
    let mut out = io::stdout().lock();
    if json {
        let metrics: serde_json::Map<String, serde_json::Value> = report
            .metrics
            .iter()
            .map(|(k, v)| (k.clone(), json!(*v)))
            .collect();
        let envelope = json!({
            "scenario": report.scenario,
            "passed":   report.passed,
            "metrics":  metrics,
            "summary":  report.summary,
        });
        let _ = writeln!(out, "{envelope}");
    } else {
        let _ = writeln!(out, "scenario: {}", report.scenario);
        let _ = writeln!(out, "passed:   {}", report.passed);
        let _ = writeln!(out, "summary:  {}", report.summary);
        if !report.metrics.is_empty() {
            let _ = writeln!(out, "metrics:");
            for (k, v) in &report.metrics {
                let _ = writeln!(out, "  {k} = {v}");
            }
        }
    }
}

/// Render the "unknown scenario" error.
///
/// Mirrors the JSON-envelope convention from `gitlore` (stable `code` string +
/// human `message`). The wire code is `unknown_scenario`.
fn emit_unknown_scenario(name: &str, json: bool) {
    if json {
        let mut out = io::stdout().lock();
        let envelope = json!({
            "error": {
                "code": "unknown_scenario",
                "message": format!("scenario `{name}` is not registered; run `gitlore-eval --list` to see the catalog"),
            }
        });
        let _ = writeln!(out, "{envelope}");
    } else {
        let mut err = io::stderr().lock();
        let _ = writeln!(
            err,
            "error: scenario `{name}` is not registered (try `gitlore-eval --list`)"
        );
    }
}

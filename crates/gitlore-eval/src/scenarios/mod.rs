//! Scenario framework for gitlore-eval.
//!
//! A *scenario* is a named, runnable evaluation that consumes fixtures (see
//! [`crate::fixtures`]) and produces a [`ScenarioReport`]. Scenarios are
//! plugged into a [`Registry`] keyed by their stable [`Scenario::name`].
//!
//! Concrete scenarios ship per milestone:
//!   * M4 / TDD-001 — search MRR + top-K.
//!   * M7 / TDD-002 — story Jaccard similarity.
//!   * M8 / TDD-003 — risk Mann-Whitney U separation.
//!
//! The trait + registry land first (this file) so the harness can be wired
//! into CI before any concrete scenarios exist.

mod builtin;
pub mod perf;

use std::collections::BTreeMap;

/// Per-scenario evaluation result.
///
/// Fields are intentionally open-ended; each scenario fills the metrics it
/// computes (e.g. `"mrr"`, `"top5"`, `"jaccard"`, `"mann_whitney_u_p"`).
/// Higher-is-better unless explicitly noted in the scenario's own docs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScenarioReport {
    /// Identifier of the scenario that produced this report. Must match
    /// [`Scenario::name`] of the producing scenario.
    pub scenario: String,
    /// `true` only when the scenario's acceptance threshold was met.
    /// Stubbed scenarios should set `false` and explain why in `summary`.
    pub passed: bool,
    /// Free-form metric map. Stable across milestones; new metrics may be
    /// added but never silently renamed.
    pub metrics: BTreeMap<String, f64>,
    /// One-line human-readable summary suitable for CI logs.
    pub summary: String,
}

impl ScenarioReport {
    /// Construct an empty report for `scenario`. `passed` defaults to `false`
    /// so a scenario that forgets to set a result is treated as a failure.
    pub fn new(scenario: impl Into<String>) -> Self {
        Self {
            scenario: scenario.into(),
            passed: false,
            metrics: BTreeMap::new(),
            summary: String::new(),
        }
    }

    /// Set a metric value, replacing any prior entry under the same key.
    pub fn with_metric(mut self, key: impl Into<String>, value: f64) -> Self {
        self.metrics.insert(key.into(), value);
        self
    }

    /// Set the human-readable summary.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Mark the scenario as passed.
    pub fn passed(mut self) -> Self {
        self.passed = true;
        self
    }
}

/// A named, runnable evaluation.
///
/// Implementations should be cheap to construct and do all real work inside
/// [`Scenario::run`]. Scenarios MUST NOT panic on missing fixtures — instead,
/// return a [`ScenarioReport`] with `passed = false` and an explanatory
/// `summary` so CI degrades gracefully (see `fixtures::FixtureSet`).
pub trait Scenario: Send + Sync {
    /// Stable kebab-case identifier (e.g. `"search-mrr"`, `"story-jaccard"`,
    /// `"risk-mann-whitney"`). Used as the registry key. Must not change once
    /// a scenario ships, because eval reports reference it by name.
    fn name(&self) -> &'static str;

    /// Execute the scenario.
    fn run(&self) -> ScenarioReport;
}

/// Name-indexed scenario registry.
///
/// Stored in a `BTreeMap` so iteration order is deterministic, which keeps
/// CI logs diff-friendly across runs.
pub struct Registry {
    scenarios: BTreeMap<&'static str, Box<dyn Scenario>>,
}

impl Registry {
    /// Construct an empty registry. Use [`default_registry`] to obtain the
    /// canonical seeded registry once milestones ship.
    pub fn new() -> Self {
        Self {
            scenarios: BTreeMap::new(),
        }
    }

    /// Insert `scenario` under its [`Scenario::name`].
    ///
    /// # Panics
    /// Panics in debug builds if a scenario with the same name is already
    /// registered. In release builds the new entry silently overwrites the
    /// old one; this is a defensive choice so a duplicate registration in
    /// production never aborts a CI run.
    pub fn register(&mut self, scenario: Box<dyn Scenario>) {
        let name = scenario.name();
        debug_assert!(
            !self.scenarios.contains_key(name),
            "duplicate scenario registration: {name}"
        );
        self.scenarios.insert(name, scenario);
    }

    /// Look up a scenario by its stable name.
    pub fn get(&self, name: &str) -> Option<&dyn Scenario> {
        self.scenarios.get(name).map(|b| b.as_ref())
    }

    /// Iterate registered names in stable (alphabetical) order.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.scenarios.keys().copied()
    }

    /// Number of registered scenarios.
    pub fn len(&self) -> usize {
        self.scenarios.len()
    }

    /// `true` when no scenarios are registered.
    pub fn is_empty(&self) -> bool {
        self.scenarios.is_empty()
    }

    /// Run every registered scenario, in name-order, and return their reports.
    pub fn run_all(&self) -> Vec<ScenarioReport> {
        self.scenarios.values().map(|s| s.run()).collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the canonical registry seeded with every milestone-ready scenario.
///
/// Today this is empty; concrete scenarios get plugged in by milestone:
///   * M4 / TDD-001 — `search-mrr`, `search-top-k`.
///   * M7 / TDD-002 — `story-jaccard`.
///   * M8 / TDD-003 — `risk-mann-whitney`.
///
/// Returning an empty registry is intentional rather than `unimplemented!()`
/// so the harness wiring can be exercised in CI before any scenario lands.
pub fn default_registry() -> Registry {
    Registry::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopScenario;
    impl Scenario for NoopScenario {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn run(&self) -> ScenarioReport {
            ScenarioReport::new("noop")
                .with_summary("noop ok")
                .with_metric("count", 0.0)
                .passed()
        }
    }

    struct AlphaScenario;
    impl Scenario for AlphaScenario {
        fn name(&self) -> &'static str {
            "alpha"
        }
        fn run(&self) -> ScenarioReport {
            ScenarioReport::new("alpha")
        }
    }

    #[test]
    fn registers_and_retrieves_by_name() {
        let mut r = Registry::new();
        r.register(Box::new(NoopScenario));
        assert_eq!(r.len(), 1);
        let s = r.get("noop").expect("noop should be registered");
        assert_eq!(s.name(), "noop");
        let report = s.run();
        assert!(report.passed);
        assert_eq!(report.scenario, "noop");
        assert_eq!(report.summary, "noop ok");
        assert_eq!(report.metrics.get("count"), Some(&0.0));
    }

    #[test]
    fn missing_scenario_returns_none() {
        let r = Registry::new();
        assert!(r.get("missing").is_none());
    }

    #[test]
    fn names_are_sorted_for_deterministic_ci_output() {
        let mut r = Registry::new();
        r.register(Box::new(NoopScenario));
        r.register(Box::new(AlphaScenario));
        let names: Vec<&str> = r.names().collect();
        assert_eq!(names, vec!["alpha", "noop"]);
    }

    #[test]
    fn run_all_returns_reports_in_name_order() {
        let mut r = Registry::new();
        r.register(Box::new(NoopScenario));
        r.register(Box::new(AlphaScenario));
        let reports = r.run_all();
        let order: Vec<&str> = reports.iter().map(|r| r.scenario.as_str()).collect();
        assert_eq!(order, vec!["alpha", "noop"]);
    }

    #[test]
    fn default_registry_is_empty_until_milestones_ship() {
        let r = default_registry();
        assert!(
            r.is_empty(),
            "default registry stays empty until M4/M7/M8 scenarios land"
        );
    }

    #[test]
    fn scenario_report_builder_composes() {
        let r = ScenarioReport::new("s")
            .with_metric("mrr", 0.75)
            .with_metric("top5", 0.9)
            .with_summary("ok")
            .passed();
        assert!(r.passed);
        assert_eq!(r.summary, "ok");
        assert_eq!(r.metrics.len(), 2);
        assert_eq!(r.metrics.get("mrr"), Some(&0.75));
    }
}

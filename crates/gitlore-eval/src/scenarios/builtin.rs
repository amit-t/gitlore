//! M2 built-in scenario catalog.
//!
//! Adds [`Registry::with_builtin_scenarios`] which seeds the registry with
//! every named scenario that will eventually ship across the milestone plan
//! (search M4 / M11, story M7, risk M8, perf M3+..M9+). At M2 each entry is a
//! [`StubScenario`] whose [`Scenario::run`] returns a passing
//! [`ScenarioReport`]; the `summary` line carries the milestone + TDD spec so
//! `gitlore-eval --list` (and the per-scenario report) make the work remaining
//! visible.
//!
//! `passed: true` is intentional. The eval-regression CI lane (see ADR-028)
//! must stay green while the harness is being wired in. The gate flips to
//! `false` for any given scenario the moment its real implementation lands —
//! at that point the stub is replaced wholesale, not edited.
//!
//! The pre-existing [`default_registry`](super::default_registry) is left
//! intact: it remains "the empty registry of really-implemented scenarios"
//! and grows as milestones ship real (non-stub) implementations. The two
//! constructors are deliberately separate so a future caller can ask for
//! "just the production catalog" without the stubs.

use super::{Registry, Scenario, ScenarioReport};

/// Single-string-pair stub backing every entry in [`BUILTINS`].
///
/// Constructed via [`StubScenario::new`] (const) so the catalog can live in
/// a static table. The `tracking` string is the trailing half of the summary
/// line, e.g. `"M4 via TDD-001"`.
struct StubScenario {
    name: &'static str,
    tracking: &'static str,
}

impl StubScenario {
    const fn new(name: &'static str, tracking: &'static str) -> Self {
        Self { name, tracking }
    }
}

impl Scenario for StubScenario {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run(&self) -> ScenarioReport {
        ScenarioReport::new(self.name)
            .with_summary(format!("stub: implementation lands at {}", self.tracking))
            .passed()
    }
}

/// Canonical M2 stub catalog.
///
/// Order in this table is human-friendly (grouped by pillar); the registry
/// itself reorders alphabetically so CI logs stay diff-friendly.
///
/// Naming convention: `<pillar>.<fixture>[.<variant>]` where `<pillar>` is one
/// of `search` / `story` / `risk` / `perf`. The dotted-name contract is
/// asserted in `tests/registry_smoke.rs`.
const BUILTINS: &[(&str, &str)] = &[
    // Search pillar (M4 + M11 hybrid extension).
    ("search.api-nodejs", "M4 via TDD-001"),
    ("search.api-nodejs.hybrid", "M11 via TDD-005"),
    // Story pillar (M7).
    ("story.golden", "M7 via TDD-002"),
    // Risk pillar (M8).
    ("risk.spicy-boring", "M8 via TDD-003"),
    // Perf budgets — one per pillar so each milestone owns its own budget walk.
    ("perf.cold_index_api_nodejs", "M3 via TDD-004"),
    ("perf.search_warm", "M4 via TDD-004"),
    ("perf.story_since", "M7 via TDD-004"),
    ("perf.risk_since", "M8 via TDD-004"),
    ("perf.hotspots_path", "M9 via TDD-004"),
];

impl Registry {
    /// Build a registry pre-populated with the M2 stub catalog.
    ///
    /// Every returned scenario currently passes; see the module header for
    /// why. Concrete implementations replace each stub one-for-one as the
    /// matching milestone ships, with no churn on the registry surface.
    pub fn with_builtin_scenarios() -> Self {
        let mut r = Self::new();
        for &(name, tracking) in BUILTINS {
            r.register(Box::new(StubScenario::new(name, tracking)));
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_holds_every_table_entry() {
        let r = Registry::with_builtin_scenarios();
        assert_eq!(r.len(), BUILTINS.len());
        for &(name, _) in BUILTINS {
            assert!(
                r.get(name).is_some(),
                "expected `{name}` registered, got None"
            );
        }
    }

    #[test]
    fn every_stub_run_passes_and_carries_tracking_tag() {
        let r = Registry::with_builtin_scenarios();
        for &(name, tracking) in BUILTINS {
            let report = r.get(name).expect("registered").run();
            assert!(report.passed, "stub {name} should be passing at M2");
            assert_eq!(report.scenario, name);
            assert!(report.metrics.is_empty(), "stubs ship no metrics");
            assert!(
                report.summary.contains(tracking),
                "{name} summary {:?} should mention tracking tag {tracking:?}",
                report.summary
            );
            assert!(report.summary.starts_with("stub:"));
        }
    }

    #[test]
    fn names_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for &(name, _) in BUILTINS {
            assert!(seen.insert(name), "duplicate name in BUILTINS: {name}");
        }
    }

    #[test]
    fn does_not_perturb_default_registry() {
        // `with_builtin_scenarios` is opt-in; the empty `default_registry`
        // contract (used as a clean slate by future production scenarios)
        // must keep behaving the same.
        assert!(super::super::default_registry().is_empty());
    }
}

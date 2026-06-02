//! Smoke test for the M2 built-in scenario catalog.
//!
//! Locks two things future PRs must not silently break:
//!
//! 1. The exact count of registered builtin scenarios (9 at M2). If a real
//!    scenario lands and *replaces* a stub the count stays at 9; if a real
//!    scenario lands and *adds* a new entry, this test must be updated in the
//!    same PR (forcing the catalog change to be a conscious decision).
//!
//! 2. The dotted-name contract — every registered name follows
//!    `<pillar>.<fixture>[.<variant>]` where pillar is one of `search`,
//!    `story`, `risk`, or `perf`. Locking the convention here means
//!    `gitlore-eval --list` output stays parseable for the self-hosted
//!    eval-regression lane (ADR-028).

use gitlore_eval::scenarios::Registry;

/// Pillars allowed as the first dotted segment of a scenario name.
///
/// Mirrors the four eval pillars from SPEC-001 §20 (search, story, risk) plus
/// `perf` for per-pillar perf-budget walks. Extending this list is a
/// deliberate decision and should ship with the milestone that introduces
/// the new pillar.
const ALLOWED_PILLARS: &[&str] = &["search", "story", "risk", "perf"];

/// Scenarios whose M2 stub has been replaced by a real implementation. They
/// are exempt from the `summary.starts_with("stub:")` assertion below; their
/// own modules carry per-scenario coverage.
const REAL_SCENARIOS: &[&str] = &["perf.cold_index_api_nodejs"];

#[test]
fn builtin_registry_has_exactly_nine_scenarios() {
    let r = Registry::with_builtin_scenarios();
    assert_eq!(
        r.names().count(),
        9,
        "M2 stub catalog must register exactly 9 scenarios; got {}",
        r.names().count()
    );
}

#[test]
fn every_builtin_name_follows_dotted_pillar_fixture_pattern() {
    let r = Registry::with_builtin_scenarios();
    for name in r.names() {
        let parts: Vec<&str> = name.split('.').collect();

        assert!(
            parts.len() >= 2,
            "scenario `{name}` must have at least two dotted segments \
             (`<pillar>.<fixture>`)"
        );

        for part in &parts {
            assert!(
                !part.is_empty(),
                "scenario `{name}` has an empty dotted segment"
            );
            assert!(
                part.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'),
                "scenario `{name}` has an invalid character in segment `{part}`: \
                 expected [a-z0-9_-]+"
            );
        }

        let pillar = parts[0];
        assert!(
            ALLOWED_PILLARS.contains(&pillar),
            "scenario `{name}` uses unknown pillar `{pillar}`; \
             allowed pillars: {ALLOWED_PILLARS:?}"
        );
    }
}

#[test]
fn every_builtin_runs_and_emits_a_stub_summary() {
    // Cross-check that each registered stub honours the "stub: …" summary
    // contract documented in `gitlore-eval --list`. Pairs with the per-pillar
    // dotted-name lock above so a stub introduced without a tracking tag
    // (or a stub that quietly flips to `passed: false`) is caught here.
    //
    // Scenarios listed in `REAL_SCENARIOS` have replaced their stub with a
    // real implementation; this test only verifies they are still
    // retrievable, runnable, and return a report keyed to their own name.
    // The summary contract is enforced per-scenario in their own modules.
    let r = Registry::with_builtin_scenarios();
    for name in r.names() {
        let scenario = r.get(name).expect("listed names must be retrievable");
        let report = scenario.run();
        assert_eq!(report.scenario, name);

        if REAL_SCENARIOS.contains(&name) {
            continue;
        }

        assert!(
            report.passed,
            "stub `{name}` must pass at M2 (real impl flips this when it ships)"
        );
        assert!(
            report.summary.starts_with("stub:"),
            "stub `{name}` summary must start with `stub:`; got {:?}",
            report.summary
        );
        assert!(
            report.summary.contains("TDD-"),
            "stub `{name}` summary should carry its tracking TDD tag; got {:?}",
            report.summary
        );
    }
}

//! Per-pillar perf-budget scenarios (SPEC-001 §7.3, TDD-004).
//!
//! Each scenario in this submodule replaces a stub row of the M2 catalog
//! one milestone at a time. The first to land is
//! [`cold_index::ColdIndexApiNodejs`] (M3-7e), guarding the cold-index
//! wall-clock budget for the `api-nodejs` fixture.
//!
//! Perf scenarios are gated by the **private** fixture root
//! (`qa/fixtures-private/`) so the public hosted CI lane stays fast and
//! reproducible. On the self-hosted eval-regression lane (ADR-028) the
//! fixture is present and the gate is active. When the fixture is absent
//! the scenario emits a passing `ScenarioReport` with a `skipped: ...`
//! summary so the public lane never goes red on missing data.

pub mod cold_index;

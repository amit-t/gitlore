//! gitlore-eval — internal evaluation harness for gitlore search, story, and
//! risk quality.
//!
//! Crate layout:
//!   * [`scenarios`] — `Scenario` trait + name-indexed registry.
//!   * [`fixtures`]  — public + (env-gated) private fixture loaders.
//!   * [`metrics`]   — stubs for MRR, top-K precision, Jaccard, Mann-Whitney U.
//!     Concrete implementations land milestone-by-milestone (M4 / M7 / M8) via
//!     TDD-001..003.
//!
//! This crate is read-only with respect to repositories under evaluation and
//! is never published to crates.io (`publish = false`).

#![deny(missing_docs)]

pub mod fixtures;
pub mod metrics;
pub mod scenarios;

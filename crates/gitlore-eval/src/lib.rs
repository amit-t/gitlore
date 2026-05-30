//! gitlore-eval: evaluation harness for gitlore.
//!
//! Re-exports the three pillars consumed by the `gitlore-eval` CLI, criterion
//! benches, and CI lanes:
//!
//! * [`scenarios`] — named, runnable evaluations (search / story / risk).
//! * [`fixtures`]  — loaders for hand-labelled queries, story golden sets,
//!   and risk regression data. Private fixtures sit behind `eval-private`.
//! * [`metrics`]   — MRR / nDCG@k for search, Jaccard for story grouping,
//!   and helpers for risk-rank comparisons.

pub mod fixtures;
pub mod metrics;
pub mod scenarios;

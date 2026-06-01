//! gitlore-core — workspace-internal core types for `gitlore` and
//! `gitlore-eval`.
//!
//! This crate is the leaf of the workspace tier graph (ADR-005): it has zero
//! intra-workspace dependencies and is the only crate both `gitlore` (the
//! shipping bin) and `gitlore-eval` (the internal eval harness) link against.
//!
//! ```text
//!   gitlore (bin)        gitlore-eval (lib+bin)
//!         \                    /
//!          \                  /
//!           +-> gitlore-core (lib) <-+
//! ```
//!
//! ## Scope
//!
//! M1 surface only:
//!
//! * [`config`] — TOML config schema and `~/.config/gitlore/` resolution.
//! * [`error`] — workspace-wide [`enum@error::Error`] / [`error::Result`].
//! * [`log`] — `tracing` init helpers (stderr + optional rolling file sink).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)]
pub mod config;
pub mod error;
pub mod log;

pub use config::{
    Bm25Weights, ClassificationConfig, Config, ConfigError, IndexConfig, OwnershipConfig,
    RiskConfig, RiskLabelCutoffs, RiskWeights, SearchConfig, StoryConfig, Theme, TuiConfig,
};

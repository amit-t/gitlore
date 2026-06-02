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
//! M1 surface plus the M3-1 Git access seam:
//!
//! * [`config`] — TOML config schema and `~/.config/gitlore/` resolution.
//! * [`error`] — workspace-wide [`enum@error::Error`] / [`error::Result`].
//! * [`log`] — `tracing` init helpers (stderr + optional rolling file sink).
//! * [`git`] — [`git::GitProvider`] trait + CLI-shell-out backend
//!   ([`git::cli::GitCliProvider`]) and ref-enumeration helpers
//!   ([`git::refs`]). M3 foundation for the indexer.
//! * [`index`] — SPEC-001 §5.1/§5.2 SQLite index: row mirrors
//!   ([`index::schema`]), versioned migration runner
//!   ([`index::migrations`]), and the index-path resolver
//!   ([`index::storage`]). M3-2 foundation for the indexer.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[allow(missing_docs)]
pub mod config;
pub mod error;
pub mod git;
pub mod index;
pub mod log;
pub mod search;

pub use config::{
    Bm25Weights, ClassificationConfig, Config, ConfigError, IndexConfig, OwnershipConfig,
    RiskConfig, RiskLabelCutoffs, RiskWeights, SearchConfig, StoryConfig, Theme, TuiConfig,
};
pub use log::{init_logging, LogGuard, LogLevel};
pub use search::{Factors, Filters, Query, SearchHit, SearchMode, SearchOrchestrator};

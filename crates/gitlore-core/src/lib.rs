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
//! * [`config`] — TOML config schema, defaults, and `~/.config/gitlore/`
//!   resolution (`directories`-backed). Stubbed; real loader lands with M10.
//! * [`error`] — top-level [`enum@error::Error`] / [`error::Result`] used
//!   across the workspace. `thiserror`-based so call sites stay terse and
//!   surfaces stay non-panicky (spec §8 / §11).
//! * [`log`] — `tracing` initialization helpers (stderr in dev, rolling file
//!   appender under XDG state dir in release).
//!
//! Future modules (storage, index, search, story, risk, hotspots, git) land
//! milestone-by-milestone — see `gitlore_unified_spec.md` §20.
//!
//! ## Features
//!
//! * `embeddings` (off) — gates `fastembed` + `sqlite-vec` for the optional
//!   semantic layer. Toggled by `gitlore setup-embeddings` at runtime (spec
//!   §4.4 / §22 row 6).
//! * `git2` (off) — Phase 3 reservation per OQ-T-3. Lets us swap the CLI
//!   `GitRepo` backend for a `git2-rs`-backed one without a breaking
//!   feature-name collision downstream.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod log;

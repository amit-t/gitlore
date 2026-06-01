//! On-disk SQLite index for gitlore (M3-2, SPEC-001 §5.1 / §5.2, TDD-000 §2.2).
//!
//! This module is the persistence seam for the indexer, search, story, risk,
//! and hotspot engines. It owns:
//!
//! * [`schema`] — Rust mirrors of the SQL tables defined in SPEC-001 §5.1.
//!   Every row type derives `serde::{Serialize, Deserialize}` for round-trip
//!   into the eval harness and the `--json` CLI surface. Columns the SQL
//!   side declares as JSON are carried as `String` in Rust and parsed/
//!   serialised through the `parse_*` / `serialize_*` helpers documented on
//!   each row type.
//! * [`migrations`] — versioned migration runner. `migrations::migrate`
//!   embeds each numbered SQL file via `include_str!`, applies new
//!   migrations inside a single transaction, and writes the resulting
//!   `schema_version` into `index_state`. On a fresh database it runs
//!   migration `0001_init.sql` unconditionally; on an existing database it
//!   reads `index_state.schema_version` and refuses to downgrade. M3-2 ships
//!   migration 0001 only (`commit_vectors` lands at M11 via
//!   `gitlore setup-embeddings`).
//! * [`storage`] — index path resolver per SPEC-001 §5.2. Prefers
//!   `<common-dir>/gitlore/` (so the index travels with the repo's bare
//!   storage and is shared across worktrees), falling back to the XDG
//!   per-user location when the common dir is read-only (Q15b, ADR-029).
//!
//! ## Wiring
//!
//! Upstream callers (the indexer, the TUI store, the eval harness) open a
//! `rusqlite::Connection` against `storage::resolve_index_path` and then
//! call `migrations::migrate` exactly once before issuing reads or writes.
//! `migrate` is idempotent — calling it on an already-current database is a
//! no-op that returns the current `schema_version`.

pub mod migrations;
pub mod schema;
pub mod storage;

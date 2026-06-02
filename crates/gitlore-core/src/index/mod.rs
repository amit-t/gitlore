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
//! * [`lock`] — writer lock + WAL checkpoint helpers (TDD-000 §2.2,
//!   SPEC-001 §3.4 / §4.5, ADR-004). One writer at a time via
//!   `fs2`-backed advisory locking with a `<pid>\n<rfc3339>\n`
//!   diagnostic payload; `wal_checkpoint_if_large` keeps the WAL from
//!   inflating during long indexer sessions.
//! * [`identity`] — three-link identity resolver (M3-4, TDD-000 §2.2,
//!   ADR-017). [`identity::OverrideResolver`] →
//!   [`identity::MailmapResolver`] → [`identity::UnionFindResolver`],
//!   composed by [`identity::ChainedResolver`]; persists into the
//!   `identities` + `identity_aliases` tables introduced at M3-2.
//! * [`classify`] — Q14 file classifier (M3-5, TDD-000 §2.2,
//!   SPEC-001 §4.4, ADR-018). [`classify::Classifier::default_for`]
//!   loads embedded defaults + ecosystem overlays detected at the repo
//!   root, then [`classify::Classifier::classify`] maps a repo-relative
//!   path to a single [`classify::Category`].
//! * [`reclassify`] — bulk re-classification pass for the indexed
//!   `commits` table. [`reclassify::reclassify_all`] walks every row,
//!   re-runs the classifier over each commit's `files_changed`, and
//!   updates the per-category counters in a single transaction.
//! * [`indexer`] — M3-6 walker engine. [`indexer::Indexer`] composes
//!   the provider, schema, lock, identity, and classify layers into a
//!   resumable walk + persistence pipeline with per-ref watermarks,
//!   revert detection, force-push retention, and chunked writes.
//! * [`identities_report`] — M3-7b read-only reader that powers
//!   `gitlore identities`. Aggregates clustered identities with alias
//!   counts and authored-commit counts via `SQLITE_OPEN_READ_ONLY` so
//!   concurrent indexer writes are not contended.
//! * [`classify_report`] — M3-7b read-only reporters that power
//!   `gitlore classify`. Either classifies a caller-supplied path list
//!   (the glob form) or resolves a SHA against `commits.files_changed`
//!   and classifies the touched files (the `--explain` form).
//!
//! ## Wiring
//!
//! Upstream callers (the indexer, the TUI store, the eval harness) open a
//! `rusqlite::Connection` against `storage::resolve_index_path` and then
//! call `migrations::migrate` exactly once before issuing reads or writes.
//! `migrate` is idempotent — calling it on an already-current database is a
//! no-op that returns the current `schema_version`.

pub mod classify;
pub mod classify_report;
pub mod identities_report;
pub mod identity;
pub mod indexer;
pub mod lock;
pub mod migrations;
pub mod reclassify;
pub mod schema;
pub mod status;
pub mod storage;

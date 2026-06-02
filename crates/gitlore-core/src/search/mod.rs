//! Search module (TDD-001 §2.1 + SPEC-001 §4.3.1).
//!
//! Foundational submodules for the M4 lexical search engine:
//!
//! * [`conn_pool`] — single read-only SQLite connection (TDD-001 §2.1 grill #17).
//! * [`path_relevance`] — directory-prefix path relevance scoring.
//! * [`recency`] — half-life exponential decay scoring.
//!
//! Submodules expose their own types; callers use the qualified path
//! (e.g. `search::recency::score`, `search::path_relevance::score`,
//! `search::conn_pool::SearchConnPool`) to avoid naming collisions.

pub mod conn_pool;
pub mod path_relevance;
pub mod recency;

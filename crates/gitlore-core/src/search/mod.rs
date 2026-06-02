//! Search module (TDD-001 §2.1 + SPEC-001 §4.3.1).
//!
//! Types and logic for the M4 lexical search engine:
//!
//! * [`recency`] — half-life exponential decay scoring.
//! * [`types`] — serde-derived data shapes (Query, Filters, SearchHit, Factors,
//!   SearchMode, SearchResults, RawHit).
//!
//! Future submodules (clock, lexical, sha_lookup, filters, path_relevance,
//! orchestrator, conn_pool) will attach here as their PRs land.

pub mod recency;
pub mod types;

pub use recency::{score, score_with_system_time};
pub use types::{Factors, Filters, Query, RawHit, SearchHit, SearchMode, SearchResults};

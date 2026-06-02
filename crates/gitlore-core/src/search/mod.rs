//! Search module (TDD-001 §2.1 + SPEC-001 §4.3.1).
//!
//! M4 lexical search engine:
//!
//! * [`types`] — shared data shapes: `Query`, `Filters`, `SearchHit`, etc.
//! * [`clock`] — `Clock` trait + `SystemClock` for testable time injection.
//! * [`conn_pool`] — `SearchConnPool`, a read-only SQLite connection wrapper.
//! * [`lexical`] — FTS5 query escape + multi-field BM25 against `commits_fts`.
//! * [`sha_lookup`] — SHA-prefix bypass that skips FTS5 for hex queries.
//! * [`filters`] — SQL predicate resolution from user-supplied `Filters`.
//! * [`orchestrator`] — `SearchOrchestrator`, the public query entry point.
//! * [`path_relevance`] — directory-prefix relevance scoring.
//! * [`recency`] — half-life exponential decay recency scoring.

pub mod clock;
pub mod conn_pool;
pub mod filters;
pub mod lexical;
pub mod orchestrator;
pub mod path_relevance;
pub mod recency;
pub mod sha_lookup;
pub mod types;

pub use lexical::LexicalSearch;
pub use orchestrator::SearchOrchestrator;
pub use types::{Factors, Filters, Query, SearchHit, SearchMode, SearchResults};

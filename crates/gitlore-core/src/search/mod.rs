//! Search module (TDD-001 §2.1 + SPEC-001 §4.3.1).
//!
//! Foundational submodules for the M4 lexical search engine:
//!
//! * [`lexical`] — FTS5 query escape + multi-field BM25 against `commits_fts`.
//! * [`path_relevance`] — directory-prefix path relevance scoring.
//! * [`recency`] — half-life exponential decay scoring.
//!
//! Submodules expose their own types; callers use the qualified path
//! (e.g. `search::lexical::Fts5LexicalSearch`, `search::recency::score`,
//! `search::path_relevance::score`) to avoid naming collisions.

pub mod lexical;
pub mod path_relevance;
pub mod recency;

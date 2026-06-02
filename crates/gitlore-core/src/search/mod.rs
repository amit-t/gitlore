//! Search module (TDD-001 §2.1 + SPEC-001 §4.3.1).
//!
//! Foundational types and scoring helpers for the M4 lexical search engine:
//!
//! * [`recency`] — half-life exponential decay scoring.
//! * [`path_relevance`] — directory-prefix path relevance scoring.
//!
//! Both submodules expose their own `score` function; callers use the
//! qualified path (`search::recency::score` / `search::path_relevance::score`)
//! to disambiguate.

pub mod path_relevance;
pub mod recency;

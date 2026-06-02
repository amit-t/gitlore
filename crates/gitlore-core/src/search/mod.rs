//! Search module (TDD-001 §2.1 + SPEC-001 §4.3.1).
//!
<<<<<<< HEAD
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
=======
//! This module contains scoring and ranking functions used in commit search,
//! including recency-based time decay and FTS5-based lexical search.

pub mod lexical;
pub mod recency;

pub use lexical::{FilterClause, Fts5LexicalSearch, LexicalSearch, RawHit};
pub use recency::{score, score_with_system_time};
>>>>>>> b617339 (feat(search): add FTS5 lexical search module with query escaping and BM25 scoring)

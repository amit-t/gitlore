//! Search-related functionality for gitlore-core.
//!
//! This module contains the search engine including lexical search,
//! query parsing, filtering, and result ranking.

pub mod lexical;
pub mod orchestrator;
pub mod query;
pub mod recency;

// Re-export public API
pub use lexical::LexicalSearch;
pub use orchestrator::SearchOrchestrator;
pub use query::{Filters, Factors, Query, SearchHit, SearchMode, sha_lookup};
pub use recency::{score, score_with_system_time};

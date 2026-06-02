//! Search-related abstractions and utilities (TDD-001 §2.1).
//!
//! Foundational types for the M4 lexical search engine: scoring + ranking
//! (recency decay), time abstractions for deterministic testing (Clock), and
//! the search module surface that downstream submodules (lexical, sha_lookup,
//! filters, orchestrator) will attach to.

pub mod clock;
pub mod recency;

pub use clock::{Clock, SystemClock};
pub use recency::{score, score_with_system_time};

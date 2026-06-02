//! Search module (TDD-001 §2.1 + SPEC-001 §4.3.1).
//!
//! Foundational submodules for the M4 lexical search engine:
//!
//! * [`recency`] — half-life exponential decay scoring.
//! * [`sha_lookup`] — SHA-prefix bypass (`^[0-9a-f]{4,40}$` → primary-key scan).
//!
//! Submodules expose their own types; callers use the qualified path
//! (`search::sha_lookup::ShaLookupOutcome`, `search::recency::score`) to
//! avoid naming collisions.

pub mod recency;
pub mod sha_lookup;

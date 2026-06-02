//! Search-related functionality for gitlore-core.
//!
//! This module contains scoring and ranking functions used in commit search,
//! including recency-based time decay.

pub mod recency;

pub use recency::{score, score_with_system_time};

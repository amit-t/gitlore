//! Search evaluation scenarios (M4 / TDD-001).
//!
//! * [`synthetic`] — 10-query public fixture (always runs on hosted CI).
//! * [`api_nodejs`] — 30-query private fixture (gated on `GITLORE_EVAL_FIXTURES_PRIVATE=1`).

pub mod api_nodejs;
pub mod synthetic;

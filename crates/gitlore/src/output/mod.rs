//! Output rendering for gitlore CLI (ADR-030 / SPEC-001 §4.3).
//!
//! Two surfaces:
//! * [`json`] — single-line JSON envelope (`{"schema_version":1,"data":...}`)
//!   and a matching error envelope.
//! * [`human`] — terminal-friendly tabular rendering for search hits.

pub mod human;
pub mod json;

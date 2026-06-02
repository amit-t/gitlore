//! Output formatting for CLI commands.
//!
//! This module provides formatting backends for different output modes:
//!
//! * [`json`] — Machine-readable JSON output per SPEC-001 §4.3
//! * [`human`] — Human-readable terminal output with colour and formatting
//!
//! Module ownership (per ADR-005 / fix_plan):
//!
//! * [`json`] — JSON serialization and error envelope rendering
//! * [`human`] — Terminal-aware pretty-printing with anstream

pub mod json;
pub mod human;

//! gitlore — local-first, narrative TUI for repo intelligence.
//!
//! This library surface holds the bin's reusable pieces so integration
//! tests (e.g. `tests/tui_launch_smoke.rs`) and future in-process consumers
//! can reach `cli::run` and the TUI app shell without re-spawning the
//! binary.
//!
//! Module ownership (per ADR-005 / fix_plan M1):
//!
//! * [`cli`] — clap-derive subcommand surface and the async `run` entry
//!   point dispatched from `main`. Scaffolded in its own M1 task; only the
//!   module declaration lives here.
//! * [`tui`] — ratatui application shell, mode switcher, and per-mode
//!   skeleton (Search / Story / Risk / Hotspots).
//! * [`output`] — formatting backends for JSON and human-readable output.
//!
//! No business logic lives at the crate root. The bin's `main.rs` owns
//! process concerns (tokio runtime, correlation id, top-level span,
//! exit-code translation) and delegates everything else here.

pub mod cli;
pub mod tui;
pub mod output;

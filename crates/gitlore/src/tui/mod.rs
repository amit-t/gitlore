//! gitlore TUI layer.
//!
//! ratatui + crossterm. Four modes (Search / Story / Risk / Hotspots)
//! switched with `Tab` / `Shift-Tab` per AC-TUI-1. `q` quits.
//!
//! Sub-modules are intentionally thin at scaffold time; real rendering and
//! key handling land in milestone M5 (TUI wired to search) and beyond.

pub mod app;
pub mod diff;
pub mod keys;
pub mod modes;
pub mod theme;

pub use app::App;
pub use modes::Mode;

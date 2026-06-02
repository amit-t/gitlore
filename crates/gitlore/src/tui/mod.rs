//! gitlore TUI layer.
//!
//! ratatui + crossterm. Four modes (Search / Story / Risk / Hotspots)
//! switched with `Tab` / `Shift-Tab` per AC-TUI-1. `q` quits.
//!
//! M5 wires the search mode to the FTS5 lexical backend, adds the diff pane,
//! modal keybindings (Nav/Input), the help overlay, and theme resolution.

pub mod app;
pub mod diff;
pub mod help;
pub mod keys;
pub mod modes;
pub mod theme;

pub use app::App;
pub use modes::Mode;

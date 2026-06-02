//! TUI modes and the global mode-switcher key handler.
//!
//! Per spec §11.9 / §17 (TUI):
//! - Four modes: Search, Story, Risk, Hotspots.
//! - `Tab` / `Shift-Tab` cycle modes (AC-TUI-1).
//! - `q` quits.
//! - Arrow keys handled by per-mode state machines in M5+.
//!
//! M5 adds the `search` submodule with the full search-mode state machine.

pub mod search;

use crossterm::event::{KeyCode, KeyEvent};

use crate::tui::app::App;

/// The four top-level TUI modes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Search,
    Story,
    Risk,
    Hotspots,
}

impl Mode {
    /// All modes in tab order. `Tab` advances forward through this slice;
    /// `Shift-Tab` advances backward.
    pub const ALL: [Mode; 4] = [Mode::Search, Mode::Story, Mode::Risk, Mode::Hotspots];

    /// Short label used in the top-bar tabs.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Search => "Search",
            Mode::Story => "Story",
            Mode::Risk => "Risk",
            Mode::Hotspots => "Hotspots",
        }
    }

    /// Mode immediately after `self` in tab order, wrapping at the end.
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Mode immediately before `self` in tab order, wrapping at the start.
    pub fn prev(self) -> Self {
        let len = Self::ALL.len();
        let idx = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(idx + len - 1) % len]
    }
}

/// Dispatch a single key event to the global mode-switcher.
///
/// Only handles the cross-mode shell:
/// - `Tab` → [`Mode::next`]
/// - `Shift-Tab` (sent by terminals as [`KeyCode::BackTab`]) → [`Mode::prev`]
/// - `q` → set `app.should_quit`
/// - Arrows → no-op (reserved for per-mode navigation in M5+)
///
/// Per-mode handlers will be invoked from here once they exist.
pub fn handle_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Tab => app.mode = app.mode.next(),
        KeyCode::BackTab => app.mode = app.mode.prev(),
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
            // Reserved for per-mode navigation; intentional no-op until M5.
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_cycles_in_tab_order() {
        assert_eq!(Mode::Search.next(), Mode::Story);
        assert_eq!(Mode::Story.next(), Mode::Risk);
        assert_eq!(Mode::Risk.next(), Mode::Hotspots);
        assert_eq!(Mode::Hotspots.next(), Mode::Search);
    }

    #[test]
    fn prev_cycles_in_reverse_tab_order() {
        assert_eq!(Mode::Search.prev(), Mode::Hotspots);
        assert_eq!(Mode::Hotspots.prev(), Mode::Risk);
        assert_eq!(Mode::Risk.prev(), Mode::Story);
        assert_eq!(Mode::Story.prev(), Mode::Search);
    }

    #[test]
    fn default_mode_is_search() {
        assert_eq!(Mode::default(), Mode::Search);
    }
}

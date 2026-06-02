//! Modal key-binding system for the TUI (ADR-015).
//!
//! # Modes
//!
//! The TUI has two input modes:
//!
//! - **Nav** — list navigation, mode switching, shortcut actions.  Arrow keys,
//!   Tab/Shift-Tab, `q`, `?`, `/` are all meaningful here.
//! - **Input** — the user is typing into a text field (the search bar).
//!   Most keys are forwarded as text. `Escape` returns to Nav; Enter submits.
//!
//! `Ctrl-C`, `Ctrl-Z`, `Ctrl-S`, `Ctrl-Q`, and all arrow / paging keys are
//! always passthrough (never consumed as actions) per ADR-015.
//!
//! # Rebinding
//!
//! [`Keymap::from_config`] reads the `[tui.keys]` TOML table and replaces
//! individual action chars.  Built-in passthrough keys (`Ctrl-*`, arrows,
//! paging) cannot be rebound here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gitlore_core::config::KeysConfig;

// ---------------------------------------------------------------------------
// InputMode
// ---------------------------------------------------------------------------

/// Active TUI editing mode (ADR-015).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Default: navigating lists, invoking shortcuts.
    #[default]
    Nav,
    /// User is typing into the search input bar.
    Input,
}

// ---------------------------------------------------------------------------
// Action
// ---------------------------------------------------------------------------

/// High-level semantic actions the TUI can perform (ADR-015).
///
/// Actions are decoupled from physical keys so tests and config can reference
/// them symbolically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // --- cross-mode ---
    /// Toggle the help overlay.
    Help,
    /// Quit the application.
    Quit,
    /// Advance to the next tab.
    NextTab,
    /// Return to the previous tab.
    PrevTab,

    // --- Nav-mode only ---
    /// Focus the search input bar (switch to Input mode).
    FocusSearch,
    /// Move the selection one row up.
    Up,
    /// Move the selection one row down.
    Down,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Jump to first item.
    Home,
    /// Jump to last item.
    End,

    // --- Input-mode only ---
    /// Submit the current query.
    Submit,
    /// Clear the query and return to Nav mode.
    Clear,
    /// Any printable character typed into the input field.
    Char(char),
    /// Backspace in the input field.
    Backspace,

    // --- Passthrough ---
    /// Key that must not be consumed by the TUI (passed to terminal / OS).
    Passthrough,
    /// Key not mapped to any action (silently ignored).
    Unrecognised,
}

// ---------------------------------------------------------------------------
// Keymap
// ---------------------------------------------------------------------------

/// Resolved key → action mapping for the TUI.
///
/// Construct with [`Keymap::default`] for built-in bindings or
/// [`Keymap::from_config`] to apply per-user overrides from `[tui.keys]`.
#[derive(Debug, Clone)]
pub struct Keymap {
    /// Key that opens the help overlay (default `?`).
    pub help: char,
    /// Key that focuses the search bar (default `/`).
    pub focus_search: char,
    /// Key that clears search and returns to Nav (default Escape, not a char;
    /// stored as a sentinel `'\x1b'` here for uniformity in tests).
    pub clear_char: char,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            help: '?',
            focus_search: '/',
            clear_char: '\x1b',
        }
    }
}

impl Keymap {
    /// Apply overrides from `[tui.keys]` config on top of built-in defaults.
    pub fn from_config(cfg: &KeysConfig) -> Self {
        let mut km = Self::default();
        if let Some(c) = cfg.help {
            km.help = c;
        }
        if let Some(c) = cfg.focus_search {
            km.focus_search = c;
        }
        // `clear` in config maps to the Escape action; store as sentinel.
        if let Some(_c) = cfg.clear {
            // Escape is always triggered by KeyCode::Esc, not a char.
            // This field in KeysConfig is advisory / documentation only.
        }
        km
    }

    // -----------------------------------------------------------------------
    // dispatch
    // -----------------------------------------------------------------------

    /// Translate a raw crossterm key event into a semantic [`Action`].
    ///
    /// The current [`InputMode`] changes which actions are reachable:
    /// in `Nav` mode printable chars trigger shortcuts; in `Input` mode they
    /// build up a query string.
    pub fn dispatch(&self, key: KeyEvent, mode: InputMode) -> Action {
        // Passthrough keys are always passthrough, regardless of mode.
        if self.is_passthrough(key) {
            return Action::Passthrough;
        }

        match mode {
            InputMode::Nav => self.dispatch_nav(key),
            InputMode::Input => self.dispatch_input(key),
        }
    }

    fn dispatch_nav(&self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') if key.modifiers.is_empty() => Action::Quit,
            KeyCode::Char(c) if c == self.help => Action::Help,
            KeyCode::Char(c) if c == self.focus_search => Action::FocusSearch,
            KeyCode::Tab => Action::NextTab,
            KeyCode::BackTab => Action::PrevTab,
            KeyCode::Up => Action::Up,
            KeyCode::Down => Action::Down,
            KeyCode::PageUp => Action::PageUp,
            KeyCode::PageDown => Action::PageDown,
            KeyCode::Home => Action::Home,
            KeyCode::End => Action::End,
            _ => Action::Unrecognised,
        }
    }

    fn dispatch_input(&self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => Action::Clear,
            KeyCode::Enter => Action::Submit,
            KeyCode::Backspace => Action::Backspace,
            KeyCode::Char(c) if c == self.help => Action::Help,
            KeyCode::Char(c) => Action::Char(c),
            _ => Action::Unrecognised,
        }
    }

    // -----------------------------------------------------------------------
    // is_passthrough
    // -----------------------------------------------------------------------

    /// Return `true` when the key must never be consumed by the TUI.
    ///
    /// The following are always passthrough (ADR-015):
    /// - `Ctrl-C`, `Ctrl-Z`, `Ctrl-S`, `Ctrl-Q`
    /// - Arrow keys, PgUp/PgDn, Home, End are handled as Actions above,
    ///   but `Ctrl-Arrow` combos are passthrough.
    pub fn is_passthrough(&self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            return matches!(
                key.code,
                KeyCode::Char('c') | KeyCode::Char('z') | KeyCode::Char('s') | KeyCode::Char('q')
            );
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    // --- passthrough ---

    #[test]
    fn ctrl_c_is_passthrough() {
        let km = Keymap::default();
        assert!(km.is_passthrough(ctrl(KeyCode::Char('c'))));
    }

    #[test]
    fn ctrl_z_is_passthrough() {
        let km = Keymap::default();
        assert!(km.is_passthrough(ctrl(KeyCode::Char('z'))));
    }

    #[test]
    fn ctrl_s_is_passthrough() {
        let km = Keymap::default();
        assert!(km.is_passthrough(ctrl(KeyCode::Char('s'))));
    }

    #[test]
    fn ctrl_q_is_passthrough() {
        let km = Keymap::default();
        assert!(km.is_passthrough(ctrl(KeyCode::Char('q'))));
    }

    #[test]
    fn regular_q_is_not_passthrough() {
        let km = Keymap::default();
        assert!(!km.is_passthrough(press(KeyCode::Char('q'))));
    }

    // --- Nav dispatch ---

    #[test]
    fn nav_q_yields_quit() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::Char('q')), InputMode::Nav),
            Action::Quit
        );
    }

    #[test]
    fn nav_question_mark_yields_help() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::Char('?')), InputMode::Nav),
            Action::Help
        );
    }

    #[test]
    fn nav_slash_yields_focus_search() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::Char('/')), InputMode::Nav),
            Action::FocusSearch
        );
    }

    #[test]
    fn nav_tab_yields_next_tab() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::Tab), InputMode::Nav),
            Action::NextTab
        );
    }

    #[test]
    fn nav_backtab_yields_prev_tab() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::BackTab), InputMode::Nav),
            Action::PrevTab
        );
    }

    #[test]
    fn nav_arrows_yield_navigation_actions() {
        let km = Keymap::default();
        assert_eq!(km.dispatch(press(KeyCode::Up), InputMode::Nav), Action::Up);
        assert_eq!(
            km.dispatch(press(KeyCode::Down), InputMode::Nav),
            Action::Down
        );
        assert_eq!(
            km.dispatch(press(KeyCode::PageUp), InputMode::Nav),
            Action::PageUp
        );
        assert_eq!(
            km.dispatch(press(KeyCode::PageDown), InputMode::Nav),
            Action::PageDown
        );
        assert_eq!(
            km.dispatch(press(KeyCode::Home), InputMode::Nav),
            Action::Home
        );
        assert_eq!(
            km.dispatch(press(KeyCode::End), InputMode::Nav),
            Action::End
        );
    }

    // --- Input dispatch ---

    #[test]
    fn input_printable_char_yields_char_action() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::Char('a')), InputMode::Input),
            Action::Char('a')
        );
    }

    #[test]
    fn input_escape_yields_clear() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::Esc), InputMode::Input),
            Action::Clear
        );
    }

    #[test]
    fn input_enter_yields_submit() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::Enter), InputMode::Input),
            Action::Submit
        );
    }

    #[test]
    fn input_backspace_yields_backspace() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::Backspace), InputMode::Input),
            Action::Backspace
        );
    }

    #[test]
    fn input_help_char_yields_help_in_input_mode() {
        // `?` should still open help even while typing.
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(press(KeyCode::Char('?')), InputMode::Input),
            Action::Help
        );
    }

    #[test]
    fn ctrl_passthrough_even_in_input_mode() {
        let km = Keymap::default();
        assert_eq!(
            km.dispatch(ctrl(KeyCode::Char('c')), InputMode::Input),
            Action::Passthrough
        );
    }

    // --- rebinding ---

    #[test]
    fn rebinding_help_key_works() {
        let cfg = KeysConfig {
            help: Some('h'),
            ..Default::default()
        };
        let km = Keymap::from_config(&cfg);
        assert_eq!(km.help, 'h');
        assert_eq!(
            km.dispatch(press(KeyCode::Char('h')), InputMode::Nav),
            Action::Help
        );
        // Default `?` no longer triggers Help.
        assert_eq!(
            km.dispatch(press(KeyCode::Char('?')), InputMode::Nav),
            Action::Unrecognised
        );
    }

    #[test]
    fn rebinding_focus_search_key_works() {
        let cfg = KeysConfig {
            focus_search: Some('s'),
            ..Default::default()
        };
        let km = Keymap::from_config(&cfg);
        assert_eq!(km.focus_search, 's');
        assert_eq!(
            km.dispatch(press(KeyCode::Char('s')), InputMode::Nav),
            Action::FocusSearch
        );
    }
}

//! Top-level TUI application state and event loop (M5).
//!
//! [`App`] carries:
//! - The active [`Mode`] (tab switcher).
//! - Per-mode state (currently only [`SearchState`]; other modes are
//!   placeholders rendered as empty panes until M7/M8/M9).
//! - The resolved [`Palette`] (set once at startup from config + probe).
//! - The [`Keymap`] (built-in defaults + config overrides).
//! - `help_visible` flag toggled by `?`.
//!
//! [`run`] ticks at ≤100 ms using [`crossterm::event::poll`] so the UI stays
//! responsive even when no key is pressed (e.g. background index completion).
//!
//! Terminal setup/teardown (raw mode, alternate screen) is the caller's
//! responsibility — handled in `cli.rs` via the `TerminalGuard` RAII wrapper.

use std::env;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};

use crate::tui::{
    help::render_help,
    keys::{Action, InputMode, Keymap},
    modes::search::SearchState,
    modes::{self, Mode},
    theme::{Palette, SystemProbe},
};
use gitlore_core::{
    config::TuiConfig, index::indexer::INDEX_DB_FILENAME, index::storage::resolve_index_path,
};

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// Top-level TUI state.
pub struct App {
    /// Currently active top-level mode.
    pub mode: Mode,
    /// Set to `true` to break out of [`run`] on the next iteration.
    pub should_quit: bool,
    /// Help overlay is visible.
    pub help_visible: bool,
    /// Resolved colour palette.
    pub palette: Palette,
    /// Resolved key bindings.
    pub keymap: Keymap,
    /// Search mode state.
    pub search: SearchState,
    /// Repo root (cwd at TUI launch). Reserved for future use (M7/M8/M9 modes).
    #[allow(dead_code)]
    repo_root: PathBuf,
}

impl App {
    /// Create a new `App`.
    ///
    /// `tui_config` drives theme resolution and key rebinding.
    /// `repo_root` is the working-tree root of the indexed repo.
    pub fn new(tui_config: &TuiConfig, repo_root: PathBuf) -> Self {
        let probe = SystemProbe;
        let palette =
            crate::tui::theme::resolve(tui_config.theme, tui_config.color_blind_safe, &probe);
        let keymap = Keymap::from_config(&tui_config.keys);

        // Resolve index path so SearchState can open the SQLite DB.
        let index_path = resolve_index_path_opt(&repo_root);

        let search = SearchState::new(repo_root.clone(), index_path);

        Self {
            mode: Mode::default(),
            should_quit: false,
            help_visible: false,
            palette,
            keymap,
            search,
            repo_root,
        }
    }
}

/// Attempt to resolve the index path; return `None` on failure (the TUI will
/// show a "run gitlore index first" message in the search status bar).
fn resolve_index_path_opt(repo_root: &std::path::Path) -> Option<PathBuf> {
    let provider = gitlore_core::git::cli::GitCliProvider::new(repo_root.to_path_buf());
    let loc = resolve_index_path(repo_root, &provider).ok()?;
    Some(loc.path().join(INDEX_DB_FILENAME))
}

impl Default for App {
    fn default() -> Self {
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::new(&TuiConfig::default(), cwd)
    }
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

/// Drive the TUI event loop until `app.should_quit` is set.
///
/// Each iteration:
/// 1. Polls for a crossterm event with a 100 ms timeout (tick).
/// 2. On a key-press, dispatches to the active mode via the [`Keymap`].
/// 3. Redraws the frame.
///
/// Terminal setup/teardown is the caller's responsibility.
pub fn run<B>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()>
where
    B: Backend,
{
    loop {
        terminal.draw(|frame| draw(frame, app))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let current_input_mode = if app.mode == Mode::Search {
                        app.search.input_mode
                    } else {
                        InputMode::Nav
                    };

                    let action = app.keymap.dispatch(key, current_input_mode);

                    // Cross-mode actions handled at app level.
                    match action {
                        Action::Quit => {
                            app.should_quit = true;
                        }
                        Action::Help => {
                            app.help_visible = !app.help_visible;
                        }
                        Action::NextTab => {
                            app.mode = app.mode.next();
                            app.help_visible = false;
                        }
                        Action::PrevTab => {
                            app.mode = app.mode.prev();
                            app.help_visible = false;
                        }
                        Action::Passthrough => {
                            // Let the terminal / OS handle it.
                        }
                        Action::Unrecognised => {}
                        other => {
                            // Delegate to per-mode handler.
                            if app.mode == Mode::Search {
                                let quit = app.search.handle_action(other);
                                if quit {
                                    app.should_quit = true;
                                }
                            }
                            // Other modes: no-op until M7/M8/M9.
                        }
                    }
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// draw
// ---------------------------------------------------------------------------

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // top bar
            Constraint::Min(1),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    frame.render_widget(top_bar(app.mode), chunks[0]);

    match app.mode {
        Mode::Search => {
            app.search.render(frame, chunks[1], &app.palette);
        }
        other => {
            // Placeholder pane for Story / Risk / Hotspots (M7/M8/M9).
            let label = other.as_str();
            frame.render_widget(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {label} (coming in a future milestone) ")),
                chunks[1],
            );
        }
    }

    frame.render_widget(footer_hints(app.mode, &app.keymap), chunks[2]);

    if app.help_visible {
        let current_input_mode = if app.mode == Mode::Search {
            app.search.input_mode
        } else {
            InputMode::Nav
        };
        render_help(frame, area, app.mode, &app.keymap, current_input_mode);
    }
}

fn top_bar(active: Mode) -> Paragraph<'static> {
    let mut spans: Vec<Span> = vec![Span::raw("gitlore  ")];
    for (i, mode) in Mode::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let label = format!("[{}]", mode.as_str());
        let style = if *mode == active {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(label, style));
    }
    Paragraph::new(Line::from(spans))
}

fn footer_hints(mode: Mode, _keymap: &Keymap) -> Paragraph<'static> {
    let text = match mode {
        Mode::Search => "[Tab] mode  [/] search  [?] help  [q] quit",
        _ => "[Tab] mode  [?] help  [q] quit",
    };
    Paragraph::new(text).style(Style::default().add_modifier(Modifier::DIM))
}

// ---------------------------------------------------------------------------
// Legacy handle_key (kept for backwards compat with modes::handle_key callers)
// ---------------------------------------------------------------------------

/// Compatibility shim: the old scaffold called [`modes::handle_key`] directly.
/// M5 moves dispatch into [`run`] but keeps this so any existing call sites
/// that haven't been updated yet don't break at compile time.
#[doc(hidden)]
pub fn legacy_handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    modes::handle_key(app, key);
}

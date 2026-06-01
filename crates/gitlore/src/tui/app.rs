//! Top-level TUI application state and event loop.
//!
//! `App` holds the active [`Mode`] and a `should_quit` flag. [`run`] drives a
//! draw/event loop: it redraws on every tick, reads one crossterm event, and
//! dispatches key events to the active mode via
//! [`crate::tui::modes::handle_key`].
//!
//! The loop only handles the cross-mode shell (Tab/Shift-Tab/q, see
//! [`crate::tui::modes`]); per-mode behaviour is plumbed in M5+.

use std::io;

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};

use crate::tui::modes::{self, Mode};

/// Top-level TUI state.
///
/// Scaffold-level only. Mode-specific state (search query, selected commit,
/// scroll offsets, …) will be added per-milestone as each mode lands.
#[derive(Debug)]
pub struct App {
    /// Currently active mode (one of Search / Story / Risk / Hotspots).
    pub mode: Mode,
    /// Set to `true` to break out of [`run`] on the next iteration.
    pub should_quit: bool,
}

impl App {
    /// Create a new `App` starting in [`Mode::Search`].
    pub fn new() -> Self {
        Self {
            mode: Mode::default(),
            should_quit: false,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Drive the TUI event loop until `app.should_quit` is set.
///
/// Each iteration:
/// 1. Redraws the frame via [`draw`].
/// 2. Blocks on a single crossterm event.
/// 3. Forwards key-press events to [`modes::handle_key`].
///
/// Terminal setup/teardown (raw mode, alternate screen) is the caller's
/// responsibility — typically handled in `main` so panics can restore state
/// via a guard.
pub fn run<B>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()>
where
    B: Backend,
{
    while !app.should_quit {
        terminal.draw(|frame| draw(frame, app))?;

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                modes::handle_key(app, key);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Render the placeholder three-pane skeleton: top bar with mode tabs, an
/// empty body split into list/detail, and a footer with key hints.
///
/// Real content for each pane is filled in per-milestone (M5 onwards).
fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(top_bar(app.mode), chunks[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);

    frame.render_widget(
        Block::default().borders(Borders::ALL).title("list"),
        body[0],
    );
    frame.render_widget(
        Block::default().borders(Borders::ALL).title("detail"),
        body[1],
    );

    frame.render_widget(footer_hints(), chunks[2]);
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

fn footer_hints() -> Paragraph<'static> {
    Paragraph::new("[Tab] mode  [Shift-Tab] prev mode  [q] quit")
        .style(Style::default().add_modifier(Modifier::DIM))
}

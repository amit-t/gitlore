//! Help overlay for the TUI (AC-TUI-4).
//!
//! [`render_help`] draws a centred modal box listing the active key bindings
//! for the current mode.  It is rendered last so it sits on top of all other
//! widgets.
//!
//! The overlay uses [`ratatui::widgets::Clear`] to erase the area behind it
//! before drawing the [`ratatui::widgets::Paragraph`] border, so no bleed-
//! through from the underlying panes occurs.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::{
    keys::{InputMode, Keymap},
    modes::Mode,
};

// ---------------------------------------------------------------------------
// Binding table
// ---------------------------------------------------------------------------

/// A single row in the help overlay.
struct Binding {
    key: &'static str,
    description: &'static str,
}

impl Binding {
    const fn new(key: &'static str, description: &'static str) -> Self {
        Self { key, description }
    }
}

/// Cross-mode bindings always shown.
const GLOBAL_BINDINGS: &[Binding] = &[
    Binding::new("Tab / Shift-Tab", "switch mode"),
    Binding::new("?", "toggle this help"),
    Binding::new("q", "quit"),
];

/// Nav-mode bindings.
const NAV_BINDINGS: &[Binding] = &[
    Binding::new("/", "focus search bar"),
    Binding::new("Up / Down", "navigate list"),
    Binding::new("PgUp / PgDn", "page up / down"),
    Binding::new("Home / End", "jump to first / last"),
    Binding::new("Enter", "open selected commit"),
];

/// Input-mode bindings.
const INPUT_BINDINGS: &[Binding] = &[
    Binding::new("Enter", "submit query"),
    Binding::new("Escape", "cancel / back to Nav"),
    Binding::new("Backspace", "delete last char"),
];

// ---------------------------------------------------------------------------
// render_help
// ---------------------------------------------------------------------------

/// Draw the help overlay centred in `area`.
///
/// Call after rendering all other widgets so the overlay is on top.
pub fn render_help(
    frame: &mut Frame,
    area: Rect,
    mode: Mode,
    _keymap: &Keymap,
    input_mode: InputMode,
) {
    // Build the text body.
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Title.
    lines.push(Line::from(vec![Span::styled(
        format!(" Mode: {} ", mode.as_str()),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(""));

    // Global section.
    lines.push(Line::from(vec![Span::styled(
        "  Global",
        Style::default().add_modifier(Modifier::UNDERLINED),
    )]));
    for b in GLOBAL_BINDINGS {
        lines.push(binding_line(b));
    }
    lines.push(Line::from(""));

    // Mode-specific section.
    let mode_label = if input_mode == InputMode::Input {
        "  Input mode"
    } else {
        "  Nav mode"
    };
    lines.push(Line::from(vec![Span::styled(
        mode_label,
        Style::default().add_modifier(Modifier::UNDERLINED),
    )]));

    let mode_bindings = if input_mode == InputMode::Input {
        INPUT_BINDINGS
    } else {
        NAV_BINDINGS
    };
    for b in mode_bindings {
        lines.push(binding_line(b));
    }

    // Compute overlay size: width = 48 cols, height = lines + 2 (border).
    let overlay_width = 50u16;
    let overlay_height = (lines.len() + 2).min(area.height as usize) as u16;

    let overlay = centred_rect(overlay_width, overlay_height, area);

    // Clear the background first.
    frame.render_widget(Clear, overlay);

    // Draw the bordered paragraph.
    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help (? to close) "),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, overlay);
}

fn binding_line(b: &Binding) -> Line<'static> {
    // Pad key column to 20 chars.
    let padded_key = format!("  {:20}", b.key);
    Line::from(vec![
        Span::styled(padded_key, Style::default().fg(Color::Yellow)),
        Span::raw(b.description),
    ])
}

/// Return a [`Rect`] centred within `parent` with `width × height`.
/// Clamped so it never exceeds the parent bounds.
fn centred_rect(width: u16, height: u16, parent: Rect) -> Rect {
    let w = width.min(parent.width);
    let h = height.min(parent.height);

    // Vertical centering.
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((parent.height.saturating_sub(h)) / 2),
            Constraint::Length(h),
            Constraint::Min(0),
        ])
        .split(parent);

    // Horizontal centering.
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((parent.width.saturating_sub(w)) / 2),
            Constraint::Length(w),
            Constraint::Min(0),
        ])
        .split(vert[1]);

    horiz[1]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn make_terminal(cols: u16, rows: u16) -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(cols, rows)).unwrap()
    }

    #[test]
    fn help_overlay_renders_without_panic_nav_mode() {
        let mut terminal = make_terminal(80, 24);
        let km = Keymap::default();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_help(frame, area, Mode::Search, &km, InputMode::Nav);
            })
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();

        // Must contain the global quit hint and the mode label.
        assert!(content.contains('q'), "help must mention 'q'");
        assert!(
            content.contains("Search"),
            "help must contain current mode name"
        );
    }

    #[test]
    fn help_overlay_contains_nav_bindings_in_nav_mode() {
        let mut terminal = make_terminal(80, 30);
        let km = Keymap::default();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_help(frame, area, Mode::Search, &km, InputMode::Nav);
            })
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();

        // Nav mode should show "focus search bar" hint.
        assert!(
            content.contains('/'),
            "nav help must show '/' focus-search binding"
        );
    }

    #[test]
    fn help_overlay_contains_input_bindings_in_input_mode() {
        let mut terminal = make_terminal(80, 30);
        let km = Keymap::default();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_help(frame, area, Mode::Search, &km, InputMode::Input);
            })
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();

        // Input mode should show "Escape" or "Backspace".
        assert!(
            content.contains("Escape") || content.contains("escape") || content.contains("back"),
            "input help must show Escape / back binding; content snippet: {}",
            &content[..content.len().min(200)]
        );
    }

    #[test]
    fn centred_rect_is_inside_parent() {
        let parent = Rect::new(0, 0, 100, 50);
        let r = centred_rect(40, 20, parent);
        assert!(r.x + r.width <= parent.x + parent.width);
        assert!(r.y + r.height <= parent.y + parent.height);
    }

    #[test]
    fn centred_rect_clamps_to_parent_size() {
        let parent = Rect::new(0, 0, 10, 5);
        let r = centred_rect(200, 200, parent);
        assert_eq!(r.width, 10);
        assert_eq!(r.height, 5);
    }
}

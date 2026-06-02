//! Search mode state machine and renderer (TDD-000 §2.3 / AC-TUI-2).
//!
//! [`SearchState`] owns the query string, the ranked hit list, the selection
//! cursor, and the diff-pane scroll offset.  It is updated by the app's event
//! loop via [`SearchState::handle_action`].
//!
//! [`SearchState::render`] draws:
//! 1. A one-line input bar at the top.
//! 2. A virtualized [`ratatui::widgets::List`] of hits (40 % width).
//! 3. A diff pane on the right (60 % width) rendered via
//!    [`crate::tui::diff::render_diff`].
//!
//! The diff pane lazy-loads on selection change; errors are shown inline.

use std::path::PathBuf;
use std::sync::Arc;

use gitlore_core::config::SearchConfig;
use gitlore_core::search::clock::SystemClock;
use gitlore_core::search::conn_pool::SearchConnPool;
use gitlore_core::search::orchestrator::SearchOrchestrator;
use gitlore_core::search::types::{Filters, Query, SearchHit};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::{
    diff::{render_diff, DiffOutput},
    keys::{Action, InputMode},
    theme::Palette,
};

// ---------------------------------------------------------------------------
// SearchState
// ---------------------------------------------------------------------------

/// All state the search mode needs to render and respond to events.
pub struct SearchState {
    /// Current content of the input bar.
    pub query: String,
    /// Byte offset of the text cursor within `query`.
    pub cursor: usize,
    /// Ranked hits from the last successful search.
    pub hits: Vec<SearchHit>,
    /// Index of the selected row in `hits`.
    pub selected: Option<usize>,
    /// Diff-pane scroll offset (line index into `diff_lines`).
    pub diff_scroll: u16,
    /// Current input mode for this pane.
    pub input_mode: InputMode,
    /// Total available hits (may exceed `hits.len()` due to the query limit).
    pub total_available: u64,
    /// Last error message to display in the status bar.
    pub last_error: Option<String>,

    // --- cached diff ---
    /// SHA whose diff is currently loaded.
    diff_sha: Option<String>,
    /// Rendered diff lines (None = not yet loaded).
    diff_lines: Option<DiffOutput>,

    // --- repo context ---
    /// Working-tree root of the indexed repo.
    repo_root: PathBuf,
    /// Path to the SQLite index file (passed to SearchConnPool).
    index_path: Option<PathBuf>,
}

impl SearchState {
    /// Create an empty search state rooted at `repo_root`.
    ///
    /// `index_path` is `None` when no index is available yet (search will
    /// return an empty result with a helpful error message).
    pub fn new(repo_root: PathBuf, index_path: Option<PathBuf>) -> Self {
        Self {
            query: String::new(),
            cursor: 0,
            hits: Vec::new(),
            selected: None,
            diff_scroll: 0,
            input_mode: InputMode::Nav,
            total_available: 0,
            last_error: None,
            diff_sha: None,
            diff_lines: None,
            repo_root,
            index_path,
        }
    }

    // -----------------------------------------------------------------------
    // Event handling
    // -----------------------------------------------------------------------

    /// Apply a semantic action to this state.
    ///
    /// Returns `true` if the app should quit (should not happen in Search mode
    /// normally, but the Action enum has a Quit variant for completeness).
    pub fn handle_action(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => return true,
            Action::FocusSearch => {
                self.input_mode = InputMode::Input;
            }
            Action::Submit => {
                self.input_mode = InputMode::Nav;
                self.run_search();
            }
            Action::Clear => {
                if self.input_mode == InputMode::Input {
                    self.query.clear();
                    self.cursor = 0;
                    self.input_mode = InputMode::Nav;
                    self.hits.clear();
                    self.total_available = 0;
                    self.last_error = None;
                    self.diff_lines = None;
                    self.diff_sha = None;
                    self.selected = None;
                }
            }
            Action::Char(c) => {
                if self.input_mode == InputMode::Input {
                    self.query.insert(self.cursor, c);
                    self.cursor += c.len_utf8();
                }
            }
            Action::Backspace => {
                if self.input_mode == InputMode::Input && self.cursor > 0 {
                    // Remove the char before the cursor.
                    let before = &self.query[..self.cursor];
                    if let Some((idx, _)) = before.char_indices().next_back() {
                        self.query.remove(idx);
                        self.cursor = idx;
                    }
                }
            }
            Action::Down => {
                if !self.hits.is_empty() {
                    let next = self
                        .selected
                        .map(|s| (s + 1).min(self.hits.len() - 1))
                        .unwrap_or(0);
                    self.set_selected(next);
                }
            }
            Action::Up => {
                if !self.hits.is_empty() {
                    let prev = self.selected.map(|s| s.saturating_sub(1)).unwrap_or(0);
                    self.set_selected(prev);
                }
            }
            Action::PageDown => {
                if !self.hits.is_empty() {
                    let next = self
                        .selected
                        .map(|s| (s + 10).min(self.hits.len() - 1))
                        .unwrap_or(0);
                    self.set_selected(next);
                }
            }
            Action::PageUp => {
                if !self.hits.is_empty() {
                    let prev = self.selected.map(|s| s.saturating_sub(10)).unwrap_or(0);
                    self.set_selected(prev);
                }
            }
            Action::Home => {
                if !self.hits.is_empty() {
                    self.set_selected(0);
                }
            }
            Action::End if !self.hits.is_empty() => {
                self.set_selected(self.hits.len() - 1);
            }
            Action::End => {}
            // Help, NextTab, PrevTab handled by app.rs before calling search.
            _ => {}
        }
        false
    }

    fn set_selected(&mut self, idx: usize) {
        self.selected = Some(idx);
        self.diff_scroll = 0;
        // Invalidate diff cache when selection changes.
        let sha = self.hits.get(idx).map(|h| h.sha.clone());
        if sha != self.diff_sha {
            self.diff_sha = sha;
            self.diff_lines = None;
        }
    }

    // -----------------------------------------------------------------------
    // Search execution
    // -----------------------------------------------------------------------

    fn run_search(&mut self) {
        let Some(index_path) = &self.index_path else {
            self.last_error = Some("no index: run 'gitlore index' first".into());
            return;
        };
        let pool = match SearchConnPool::open(index_path) {
            Ok(p) => p,
            Err(e) => {
                self.last_error = Some(format!("open index: {e}"));
                return;
            }
        };
        let config = SearchConfig::default();
        let clock = Arc::new(SystemClock);
        let orch = SearchOrchestrator::new(pool, config, clock);
        let q = Query {
            text: self.query.clone(),
            filters: Filters::default(),
            limit: 50,
        };
        match orch.query(&q) {
            Ok(results) => {
                self.total_available = results.total_available;
                self.hits = results.results;
                self.selected = if self.hits.is_empty() { None } else { Some(0) };
                self.last_error = None;
                // Pre-load diff for first hit.
                if let Some(hit) = self.hits.first() {
                    self.diff_sha = Some(hit.sha.clone());
                    self.diff_lines = None;
                }
            }
            Err(e) => {
                self.last_error = Some(format!("search error: {e}"));
                self.hits.clear();
                self.total_available = 0;
                self.selected = None;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Diff lazy-load
    // -----------------------------------------------------------------------

    fn ensure_diff_loaded(&mut self, viewport_rows: u16) {
        let Some(sha) = &self.diff_sha else {
            return;
        };
        if self.diff_lines.is_some() {
            return;
        }
        let sha = sha.clone();
        let repo = self.repo_root.clone();
        match render_diff(&sha, &repo, viewport_rows) {
            Ok(out) => self.diff_lines = Some(out),
            Err(e) => {
                self.diff_lines = Some(DiffOutput {
                    lines: vec![Line::from(format!("diff error: {e}"))],
                    truncated: false,
                });
            }
        }
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    /// Render the search mode into `frame` within `area`.
    pub fn render(&mut self, frame: &mut Frame, area: ratatui::layout::Rect, palette: &Palette) {
        // Layout: [input bar 1 row] / [body] / [status 1 row]
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // input bar (with border)
                Constraint::Min(1),    // body
                Constraint::Length(1), // status line
            ])
            .split(area);

        self.render_input_bar(frame, chunks[0], palette);
        self.render_body(frame, chunks[1], palette);
        self.render_status(frame, chunks[2], palette);
    }

    fn render_input_bar(&self, frame: &mut Frame, area: ratatui::layout::Rect, _palette: &Palette) {
        let title = if self.input_mode == InputMode::Input {
            " Search (Enter to submit, Esc to cancel) "
        } else {
            " Search (/ to focus) "
        };

        let style = if self.input_mode == InputMode::Input {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let display = if self.input_mode == InputMode::Input {
            format!("{}_", self.query) // show cursor placeholder
        } else {
            self.query.clone()
        };

        let paragraph = Paragraph::new(display)
            .style(style)
            .block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(paragraph, area);
    }

    fn render_body(&mut self, frame: &mut Frame, area: ratatui::layout::Rect, palette: &Palette) {
        // Split body: list (40%) / diff (60%).
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);

        self.render_hit_list(frame, body[0], palette);

        let diff_area = body[1];
        self.ensure_diff_loaded(diff_area.height);
        self.render_diff_pane(frame, diff_area, palette);
    }

    fn render_hit_list(
        &mut self,
        frame: &mut Frame,
        area: ratatui::layout::Rect,
        _palette: &Palette,
    ) {
        let items: Vec<ListItem> = self
            .hits
            .iter()
            .map(|h| {
                let sha_short = &h.sha[..8.min(h.sha.len())];
                let text = format!("{sha_short}  {}", h.subject);
                ListItem::new(text)
            })
            .collect();

        let mut list_state = ListState::default();
        list_state.select(self.selected);

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} results ", self.hits.len())),
            )
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::REVERSED)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        frame.render_stateful_widget(list, area, &mut list_state);
    }

    fn render_diff_pane(&self, frame: &mut Frame, area: ratatui::layout::Rect, _palette: &Palette) {
        let title = match &self.diff_sha {
            Some(sha) => format!(" diff: {} ", &sha[..8.min(sha.len())]),
            None => " diff ".to_string(),
        };

        match &self.diff_lines {
            None => {
                let p = Paragraph::new("(select a hit to view diff)")
                    .block(Block::default().borders(Borders::ALL).title(title));
                frame.render_widget(p, area);
            }
            Some(out) => {
                let scroll_offset = self.diff_scroll;
                // Build visible slice.
                let lines: Vec<Line<'static>> = out
                    .lines
                    .iter()
                    .skip(scroll_offset as usize)
                    .cloned()
                    .collect();
                let truncated_note = if out.truncated {
                    vec![Line::from(Span::styled(
                        "  [diff truncated -- press D for full diff]",
                        Style::default().fg(Color::Yellow),
                    ))]
                } else {
                    vec![]
                };
                let all_lines: Vec<Line<'static>> =
                    lines.into_iter().chain(truncated_note).collect();
                let p = Paragraph::new(all_lines)
                    .block(Block::default().borders(Borders::ALL).title(title));
                frame.render_widget(p, area);
            }
        }
    }

    fn render_status(&self, frame: &mut Frame, area: ratatui::layout::Rect, _palette: &Palette) {
        let text = if let Some(err) = &self.last_error {
            format!("error: {err}")
        } else if self.hits.is_empty() && !self.query.is_empty() {
            format!("0 of {} results", self.total_available)
        } else if !self.hits.is_empty() {
            format!("{} of {} results", self.hits.len(), self.total_available)
        } else {
            String::new()
        };
        let p = Paragraph::new(text).style(Style::default().add_modifier(Modifier::DIM));
        frame.render_widget(p, area);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keys::Action;

    fn empty_state() -> SearchState {
        SearchState::new("/tmp/test-repo".into(), None)
    }

    #[test]
    fn empty_submit_shows_zero_results() {
        let mut state = empty_state();
        // Focus and submit with empty query.
        state.handle_action(Action::FocusSearch);
        state.handle_action(Action::Submit);
        assert_eq!(state.hits.len(), 0);
        // Should have an error about missing index.
        assert!(state.last_error.is_some());
        assert_eq!(state.total_available, 0);
    }

    #[test]
    fn typing_updates_query_string() {
        let mut state = empty_state();
        state.handle_action(Action::FocusSearch);
        for c in "hello".chars() {
            state.handle_action(Action::Char(c));
        }
        assert_eq!(state.query, "hello");
        assert_eq!(state.cursor, 5);
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut state = empty_state();
        state.handle_action(Action::FocusSearch);
        state.handle_action(Action::Char('a'));
        state.handle_action(Action::Char('b'));
        state.handle_action(Action::Backspace);
        assert_eq!(state.query, "a");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn clear_resets_state_and_returns_to_nav() {
        let mut state = empty_state();
        state.handle_action(Action::FocusSearch);
        state.handle_action(Action::Char('x'));
        state.handle_action(Action::Clear);
        assert_eq!(state.query, "");
        assert_eq!(state.input_mode, InputMode::Nav);
    }

    #[test]
    fn down_does_nothing_on_empty_hits() {
        let mut state = empty_state();
        state.handle_action(Action::Down);
        assert_eq!(state.selected, None);
    }

    #[test]
    fn selection_advances_on_down() {
        let mut state = empty_state();
        // Inject 3 fake hits.
        state.hits = make_hits(3);
        state.selected = Some(0);
        state.handle_action(Action::Down);
        assert_eq!(state.selected, Some(1));
        state.handle_action(Action::Down);
        assert_eq!(state.selected, Some(2));
        // Clamped at end.
        state.handle_action(Action::Down);
        assert_eq!(state.selected, Some(2));
    }

    #[test]
    fn home_end_jump_to_first_last() {
        let mut state = empty_state();
        state.hits = make_hits(5);
        state.selected = Some(2);
        state.handle_action(Action::Home);
        assert_eq!(state.selected, Some(0));
        state.handle_action(Action::End);
        assert_eq!(state.selected, Some(4));
    }

    #[test]
    fn status_shows_hit_count_when_results_exist() {
        let mut state = empty_state();
        state.hits = make_hits(3);
        state.total_available = 3;
        // Render to a small TestBackend to exercise the render path.
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let palette = crate::tui::theme::Palette::mono();
        terminal
            .draw(|frame| {
                let area = frame.area();
                state.render(frame, area, &palette);
            })
            .unwrap();
        // Just checking no panic and status includes "3".
        let buf = terminal.backend().buffer().clone();
        let text: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            text.contains('3'),
            "status should contain hit count 3; got: {}",
            &text[..200.min(text.len())]
        );
    }

    fn make_hits(n: usize) -> Vec<SearchHit> {
        (0..n)
            .map(|i| SearchHit {
                sha: format!("{i:040x}"),
                subject: format!("commit {i}"),
                author: "tester".into(),
                committed_at: 1_000_000 + i as i64,
                score: 1.0 - i as f32 * 0.1,
                factors: gitlore_core::search::types::Factors {
                    lexical_bm25: 1.0,
                    path_relevance: 0.5,
                    recency: 0.8,
                    semantic: None,
                },
            })
            .collect()
    }
}

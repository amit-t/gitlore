//! M1 scaffold CLI surface.
//!
//! At M1 the only reachable surface is the default no-arg launch into the
//! TUI shell (AC-INIT-1, AC-TUI-1). The full clap-derive subcommand surface
//! (search, story, risk, hotspots, index, …) per SPEC-001 §4.1 lands in
//! subsequent fix_plan tasks.
//!
//! `run` is invoked from `gitlore::main` inside an `info` span carrying the
//! per-invocation `correlation_id` (UUIDv7). Terminal setup and teardown
//! (raw mode, alternate screen) are owned here so panics inside the TUI
//! event loop still restore the host terminal via the RAII guard.

use std::io;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::tui::{app, App};

pub async fn run() -> io::Result<()> {
    let mut guard = TerminalGuard::install()?;
    let mut state = App::default();
    let result = app::run(&mut guard.terminal, &mut state);
    guard.restore()?;
    result
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    armed: bool,
}

impl TerminalGuard {
    fn install() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            armed: true,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

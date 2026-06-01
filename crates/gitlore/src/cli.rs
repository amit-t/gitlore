//! gitlore CLI surface (SPEC-001 §4.1).
//!
//! Full clap-derive subcommand catalog. At M1 the only branch with a real
//! implementation is the default no-arg invocation, which boots the TUI via
//! [`TerminalGuard`] (AC-INIT-1, AC-TUI-1). Every explicit subcommand is
//! parseable so `gitlore --help` reflects the eventual surface, but the
//! handler bodies return [`gitlore_core::Error::Unimplemented`] with the
//! stable wire code `"unimplemented"` (SPEC-001 §4.3).
//!
//! ## Output contract
//!
//! Errors are rendered by this module before the `Result` is handed back to
//! `main`:
//!
//! * Without `--json`, the line `error: <Display>` is written to stderr.
//! * With `--json`, the SPEC-001 §4.3 envelope
//!   `{"error":{"code":"...","message":"..."}}` is written to stdout as a
//!   single line. Stdout (not stderr) is the envelope target so scripted
//!   callers can pipe it straight into `jq` even when stderr is muted.
//!
//! `main` keeps ownership of process concerns (tokio runtime, top-level span
//! carrying the UUIDv7 `correlation_id`, exit-code translation). Terminal
//! lifecycle (raw mode, alternate screen) is owned here so panics inside the
//! TUI event loop still restore the host terminal via the RAII guard.

use std::io::{self, Write};

use clap::{Parser, Subcommand};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use gitlore_core::error::{Error, Result};
use ratatui::{backend::CrosstermBackend, Terminal};
use serde_json::json;

use crate::tui::{app, App};

/// `gitlore` — local-first, narrative TUI for repo intelligence.
///
/// Default (no subcommand) launches the TUI inside the current Git repo.
/// Explicit subcommands are plumbed per SPEC-001 §4.1; non-M1 surfaces
/// return the stable `"unimplemented"` error.
#[derive(Debug, Parser)]
#[command(
    name = "gitlore",
    version,
    about = "Local-first, narrative TUI for repo intelligence.",
    long_about = None,
    disable_help_subcommand = false,
)]
struct Cli {
    /// Emit machine-readable output (and the SPEC-001 §4.3 error envelope on
    /// failure). Available on every subcommand.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Top-level subcommand catalog (SPEC-001 §4.1).
///
/// Multi-word variants are kebab-cased by clap (`SetupEmbeddings` →
/// `setup-embeddings`). Variant payloads exist so `--help` shows real
/// positional/option help even though no handler has landed yet.
#[derive(Debug, Subcommand)]
enum Command {
    /// Index (or update) the repo's history into the local SQLite store.
    Index,

    /// Search the index lexically (and, once embeddings are set up, semantically).
    Search {
        /// Free-text query string.
        query: String,
        /// Cap the number of returned results.
        #[arg(long)]
        limit: Option<u32>,
        /// Restrict results to commits touching this path prefix.
        #[arg(long)]
        path: Option<String>,
        /// Lower bound for the commit window (ref/SHA/date).
        #[arg(long)]
        since: Option<String>,
        /// Upper bound for the commit window (ref/SHA/date).
        #[arg(long)]
        until: Option<String>,
    },

    /// Group commits into narrative stories over a window.
    Story {
        /// Lower bound for the commit window (ref/SHA/date).
        #[arg(long)]
        since: Option<String>,
        /// Upper bound for the commit window (ref/SHA/date).
        #[arg(long)]
        until: Option<String>,
        /// Restrict input to commits touching this path prefix.
        #[arg(long)]
        path: Option<String>,
        /// Cap the number of stories returned.
        #[arg(long)]
        limit: Option<u32>,
    },

    /// Score commits in a window for risk and rank them.
    Risk {
        /// Lower bound for the commit window (ref/SHA/date).
        #[arg(long)]
        since: Option<String>,
        /// Upper bound for the commit window (ref/SHA/date).
        #[arg(long)]
        until: Option<String>,
        /// Restrict scoring to commits touching this path prefix.
        #[arg(long)]
        path: Option<String>,
        /// Cap the number of ranked commits returned.
        #[arg(long)]
        limit: Option<u32>,
    },

    /// Show churn / co-change / ownership hotspots under a path.
    Hotspots {
        /// Path prefix to analyse (defaults to repo root if omitted).
        path: Option<String>,
        /// Cap the number of hotspots returned.
        #[arg(long)]
        limit: Option<u32>,
    },

    /// Explain a single commit (subject, body, diff, risk factors).
    Explain {
        /// Commit SHA or ref to inspect.
        commit: String,
    },

    /// Summarise everything between two refs (commits, authors, files, churn).
    Between {
        /// Lower bound ref/SHA (exclusive on the older side).
        from: String,
        /// Upper bound ref/SHA (inclusive).
        to: String,
    },

    /// Download and install the embedding model (opts into hybrid ranking).
    SetupEmbeddings,

    /// Inspect or modify gitlore configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Show resolved author identities (name + email aliases).
    Identities,

    /// Re-run heuristic commit classification over the index.
    Classify,

    /// Print status of the local index (path, schema version, last sync).
    Status,
}

/// `gitlore config` sub-actions.
#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Print the value of a single config key.
    Get {
        /// Dotted config key (e.g. `tui.theme`).
        key: String,
    },
    /// Set a config key to a value (writes to the per-repo override file).
    Set {
        /// Dotted config key (e.g. `tui.theme`).
        key: String,
        /// New value (parsed per the key's declared type).
        value: String,
    },
    /// List every config key and its resolved value.
    List,
}

impl Command {
    /// Kebab-case identifier used in [`Error::Unimplemented`] payloads and in
    /// the human/JSON error rendering. Stable: downstream tooling matches on
    /// this string.
    fn name(&self) -> String {
        match self {
            Command::Index => "index".into(),
            Command::Search { .. } => "search".into(),
            Command::Story { .. } => "story".into(),
            Command::Risk { .. } => "risk".into(),
            Command::Hotspots { .. } => "hotspots".into(),
            Command::Explain { .. } => "explain".into(),
            Command::Between { .. } => "between".into(),
            Command::SetupEmbeddings => "setup-embeddings".into(),
            Command::Config { action } => match action {
                ConfigAction::Get { .. } => "config get".into(),
                ConfigAction::Set { .. } => "config set".into(),
                ConfigAction::List => "config list".into(),
            },
            Command::Identities => "identities".into(),
            Command::Classify => "classify".into(),
            Command::Status => "status".into(),
        }
    }
}

/// Entry point dispatched from `main`.
///
/// Parses argv, runs the requested subcommand, and — on failure — renders the
/// human-readable line or JSON envelope before returning the error to `main`.
/// `main`'s only remaining job is to translate the `Result` into an
/// [`std::process::ExitCode`].
pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => run_tui().await,
        Some(cmd) => {
            let outcome = dispatch(&cmd);
            if let Err(ref err) = outcome {
                emit_error(err, cli.json);
            }
            outcome
        }
    }
}

/// Run the configured subcommand. At M1 every arm returns
/// [`Error::Unimplemented`]; future milestones replace these with real work.
fn dispatch(cmd: &Command) -> Result<()> {
    Err(Error::Unimplemented {
        subcommand: cmd.name(),
    })
}

/// Render an [`Error`] for the user.
///
/// `json = false` → `error: <Display>` to stderr (plain mode, the default).
/// `json = true`  → SPEC-001 §4.3 envelope to stdout as a single line.
fn emit_error(err: &Error, json: bool) {
    if json {
        let envelope = json!({
            "error": {
                "code": err.code(),
                "message": err.to_string(),
            }
        });
        // Write to stdout so `gitlore --json search foo | jq .error.code`
        // works even when stderr is muted by the caller.
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "{envelope}");
    } else {
        let mut stderr = io::stderr().lock();
        let _ = writeln!(stderr, "error: {err}");
    }
}

/// Launch the ratatui TUI inside the [`TerminalGuard`] RAII wrapper.
async fn run_tui() -> Result<()> {
    let mut guard = TerminalGuard::install()?;
    let mut state = App::default();
    let result = app::run(&mut guard.terminal, &mut state);
    guard.restore()?;
    Ok(result?)
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

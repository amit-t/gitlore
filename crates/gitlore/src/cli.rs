//! gitlore CLI surface (SPEC-001 §4.1).
//!
//! Full clap-derive subcommand catalog. M3-7 wires real handlers for
//! `gitlore index` (the indexer engine landed at M3-6) and
//! `gitlore status` (read-only index header). Every other subcommand is
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
//! Successful subcommands respect the same split: the human form prints
//! to stdout in plain text; `--json` prints exactly one JSON line to
//! stdout.
//!
//! `main` keeps ownership of process concerns (tokio runtime, top-level span
//! carrying the UUIDv7 `correlation_id`, exit-code translation). Terminal
//! lifecycle (raw mode, alternate screen) is owned here so panics inside the
//! TUI event loop still restore the host terminal via the RAII guard.

use std::env;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use gitlore_core::error::{Error, Result};
use gitlore_core::index::indexer::{IndexReport, Indexer, RefPlan};
use gitlore_core::index::lock::LockMode;
use gitlore_core::index::status::StatusReport;
use ratatui::{backend::CrosstermBackend, Terminal};
use serde_json::{json, Value};

use crate::tui::{app, App};

/// `gitlore` — local-first, narrative TUI for repo intelligence.
///
/// Default (no subcommand) launches the TUI inside the current Git repo.
/// Explicit subcommands are plumbed per SPEC-001 §4.1; non-M3-7 surfaces
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
    Index(IndexArgs),

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

/// `gitlore index` arguments (SPEC-001 §4.1).
#[derive(Debug, clap::Args)]
struct IndexArgs {
    /// Enumerate refs and estimate the commit count without touching
    /// the database. Mutually exclusive with `--rebuild`.
    #[arg(long, conflicts_with = "rebuild")]
    dry_run: bool,

    /// Drop the existing database and re-walk from scratch.
    #[arg(long)]
    rebuild: bool,

    /// Fail fast with `lock_contention` instead of waiting on a
    /// concurrent writer. Defaults to waiting (kernel-level blocking).
    #[arg(long)]
    no_wait: bool,
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
            Command::Index(_) => "index".into(),
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
            let outcome = dispatch(&cmd, cli.json);
            if let Err(ref err) = outcome {
                emit_error(err, cli.json);
            }
            outcome
        }
    }
}

/// Run the configured subcommand. Subcommands wired at M3-7 (`index`,
/// `status`) return real results; the rest still resolve to
/// [`Error::Unimplemented`] with their stable wire name.
fn dispatch(cmd: &Command, json: bool) -> Result<()> {
    match cmd {
        Command::Index(args) => run_index(args, json),
        Command::Status => run_status(json),
        other => Err(Error::Unimplemented {
            subcommand: other.name(),
        }),
    }
}

/// Handle `gitlore index [--dry-run|--rebuild] [--no-wait]`.
fn run_index(args: &IndexArgs, json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let lock_mode = if args.no_wait {
        LockMode::NoWait
    } else {
        LockMode::Wait
    };
    let mut indexer = Indexer::open(&cwd, lock_mode)?;

    if args.dry_run {
        let plan = indexer.dry_run()?;
        emit_dry_run(&plan, json);
        return Ok(());
    }

    let progress = ProgressPrinter::new(json);
    let report = if args.rebuild {
        let mut cb = progress.callback();
        indexer.rebuild(&mut cb)?
    } else if indexer.has_watermark()? {
        let mut cb = progress.callback();
        indexer.run_incremental(&mut cb)?
    } else {
        let mut cb = progress.callback();
        indexer.run_initial(&mut cb)?
    };
    progress.finish();
    emit_index_report(&report, json);
    Ok(())
}

/// Handle `gitlore status` — open the index read-only and render the
/// header (commit count, schema version, embeddings state, writer-lock
/// holder).
fn run_status(json: bool) -> Result<()> {
    let cwd = env::current_dir()?;
    let provider = gitlore_core::git::cli::GitCliProvider::new(cwd.clone());
    let report = StatusReport::read(&cwd, &provider)?;
    emit_status(&report, json);
    Ok(())
}

/// One-line-per-second stderr progress printer for the indexer walk.
///
/// Suppresses output entirely when `--json` is in effect so the JSON
/// envelope on stdout stays the only machine-parseable surface. Also
/// suppresses when stderr is not a TTY *and* the env var
/// `GITLORE_PROGRESS=always` is not set, so unit tests and piped runs
/// don't get noisy stderr.
struct ProgressPrinter {
    indexed: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ProgressPrinter {
    fn new(json: bool) -> Self {
        let suppressed = json
            || (!io::stderr().is_terminal()
                && env::var_os("GITLORE_PROGRESS")
                    .map(|v| v != "always")
                    .unwrap_or(true));
        let indexed = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let handle = if suppressed {
            None
        } else {
            let indexed_c = indexed.clone();
            let total_c = total.clone();
            let finished_c = finished.clone();
            Some(std::thread::spawn(move || {
                let start = Instant::now();
                while !finished_c.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(1));
                    if finished_c.load(Ordering::Relaxed) {
                        break;
                    }
                    let n = indexed_c.load(Ordering::Relaxed);
                    let m = total_c.load(Ordering::Relaxed);
                    let elapsed = start.elapsed().as_secs();
                    let eta = if n > 0 && m > n {
                        ((m - n) as f64) * (elapsed as f64) / (n as f64)
                    } else {
                        0.0
                    };
                    let mut err = io::stderr().lock();
                    let _ = writeln!(
                        err,
                        "indexed {n}/{m} ({elapsed} s elapsed, {eta:.0} s ETA)"
                    );
                }
            }))
        };
        Self {
            indexed,
            total,
            finished,
            handle,
        }
    }

    fn callback(&self) -> impl FnMut(u64, u64) + '_ {
        let indexed = self.indexed.clone();
        let total = self.total.clone();
        move |n, m| {
            indexed.store(n, Ordering::Relaxed);
            total.store(m, Ordering::Relaxed);
        }
    }

    fn finish(self) {
        self.finished.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle {
            let _ = h.join();
        }
    }
}

fn emit_index_report(report: &IndexReport, json: bool) {
    let watermark: serde_json::Map<String, Value> = report
        .watermark
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.as_str().to_string())))
        .collect();
    if json {
        let envelope = json!({
            "commits_indexed": report.commits_indexed,
            "commits_total": report.commits_total,
            "ref_count": report.ref_count,
            "duration_ms": report.duration_ms,
            "watermark": watermark,
        });
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "{envelope}");
    } else {
        let mut stdout = io::stdout().lock();
        let _ = writeln!(
            stdout,
            "indexed {} of {} commit(s) across {} ref(s) in {} ms",
            report.commits_indexed, report.commits_total, report.ref_count, report.duration_ms,
        );
    }
}

fn emit_dry_run(plan: &RefPlan, json: bool) {
    if json {
        let refs: Vec<Value> = plan
            .refs
            .iter()
            .map(|r| {
                json!({
                    "name": r.name,
                    "sha": r.sha.as_str(),
                    "kind": match r.ref_type {
                        gitlore_core::git::RefType::Branch => "branch",
                        gitlore_core::git::RefType::RemoteBranch => "remote_branch",
                        gitlore_core::git::RefType::Tag => "tag",
                    },
                })
            })
            .collect();
        let envelope = json!({
            "commits_indexed": 0,
            "commits_total": plan.estimated_commits,
            "ref_count": plan.refs.len(),
            "duration_ms": 0,
            "watermark": {},
            "refs": refs,
            "dry_run": true,
        });
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "{envelope}");
    } else {
        let mut stdout = io::stdout().lock();
        let _ = writeln!(
            stdout,
            "dry-run: {} ref(s), ~{} unique commit(s) reachable",
            plan.refs.len(),
            plan.estimated_commits,
        );
        for r in &plan.refs {
            let _ = writeln!(stdout, "  {} {}", r.sha.as_str(), r.name);
        }
    }
}

fn emit_status(report: &StatusReport, json: bool) {
    if json {
        let writer_lock = match &report.writer_lock {
            Some(w) => json!({"pid": w.pid, "started_at": w.started_at}),
            None => Value::Null,
        };
        let envelope = json!({
            "commit_count": report.commit_count,
            "db_path": report.db_path,
            "db_size_bytes": report.db_size_bytes,
            "schema_version": report.schema_version,
            "embeddings_enabled": report.embeddings_enabled,
            "model": report.model,
            "writer_lock": writer_lock,
        });
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "{envelope}");
    } else {
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "index: {}", report.db_path.display());
        let _ = writeln!(stdout, "schema_version: {}", report.schema_version);
        let _ = writeln!(stdout, "commits: {}", report.commit_count);
        let _ = writeln!(stdout, "db_size_bytes: {}", report.db_size_bytes);
        let _ = writeln!(
            stdout,
            "embeddings: {}{}",
            if report.embeddings_enabled {
                "enabled"
            } else {
                "disabled"
            },
            match &report.model {
                Some(m) => format!(" (model: {m})"),
                None => String::new(),
            }
        );
        match &report.writer_lock {
            Some(w) => {
                let _ = writeln!(
                    stdout,
                    "writer_lock: pid={} since={}",
                    w.pid, w.started_at
                );
            }
            None => {
                let _ = writeln!(stdout, "writer_lock: (none)");
            }
        }
    }
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


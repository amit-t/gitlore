//! `tracing` initialization for the gitlore workspace.
//!
//! Two sinks:
//!
//! * Stderr layer — gated by `RUST_LOG` (env-filter). Off-by-default at
//!   info-level for TUI cleanliness; users opt in with e.g. `RUST_LOG=debug`.
//! * Optional file appender — daily rolling log under
//!   [`crate::config::state_dir`] when a directory is supplied. Used by the
//!   M3 indexer and beyond; M1 callers can pass `None`.
//!
//! Both layers go through the same `tracing-subscriber` registry so spans
//! and structured fields stay consistent across sinks.

use std::io;
use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Handle returned from [`init`] / [`init_with_file`] that must stay alive
/// for the lifetime of the program.
///
/// `tracing-appender`'s non-blocking writer drops queued lines when the
/// guard drops, so the binary keeps this on the stack until shutdown.
#[must_use = "drop closes the log file appender; keep this alive until program exit"]
pub struct LogGuard {
    _file: Option<WorkerGuard>,
}

/// Install the stderr-only tracing subscriber.
///
/// Safe to call once at program start. Subsequent calls are a no-op (the
/// global subscriber is already set). Honours `RUST_LOG`; defaults to
/// `warn` so the TUI is not noisy on first run (spec §8).
pub fn init() -> LogGuard {
    install(None);
    LogGuard { _file: None }
}

/// Install a stderr + rolling-file tracing subscriber.
///
/// `dir` should be a writable directory — usually [`crate::config::state_dir`].
/// File appender writes one file per day named `gitlore.log.<YYYY-MM-DD>`.
pub fn init_with_file(dir: &Path) -> io::Result<LogGuard> {
    std::fs::create_dir_all(dir)?;
    let file_appender = tracing_appender::rolling::daily(dir, "gitlore.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    install(Some(non_blocking));
    Ok(LogGuard { _file: Some(guard) })
}

fn install(file: Option<tracing_appender::non_blocking::NonBlocking>) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    let stderr_layer = fmt::layer().with_writer(io::stderr).with_target(false);

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer);

    // `try_init` returns Err if a subscriber is already set (e.g. tests
    // double-initing). We intentionally swallow that — the first init wins.
    if let Some(file) = file {
        let file_layer = fmt::layer().with_writer(file).with_ansi(false);
        let _ = registry.with(file_layer).try_init();
    } else {
        let _ = registry.try_init();
    }
}

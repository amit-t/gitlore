//! gitlore binary entry point.
//!
//! Process-level responsibilities owned here:
//!
//! 1. Build a multi-threaded tokio runtime (`rt-multi-thread`, `macros`,
//!    `signal`). Tokio is on at M1 per OQ-T-5 so the indexer and
//!    crossterm's async event reader share one runtime.
//! 2. Install a baseline `tracing` subscriber on stderr so the
//!    per-invocation span has somewhere to register. The full logging
//!    stack (level routing, rolling files, structured format) lands via
//!    `gitlore_core::log::init_logging` once `cli::run` has parsed
//!    `--quiet` / `--verbose` / `RUST_LOG`.
//! 3. Generate a per-invocation `correlation_id` (UUIDv7 — sortable,
//!    time-ordered) so every log line, span, and JSON error envelope
//!    ties back to one launch.
//! 4. Open an `info` span carrying that id and instrument the call into
//!    [`gitlore::cli::run`] so downstream events inherit the id.
//! 5. Translate the `Result` from `cli::run` into a process [`ExitCode`].
//!
//! Terminal lifecycle (raw mode, alternate screen, mouse capture) is
//! owned by `cli::run` / the TUI layer — `main` is intentionally
//! display-agnostic so the same entry point serves the headless
//! subcommands (`gitlore index`, `gitlore search`, …) and the default
//! TUI launch.

use std::process::ExitCode;

use tracing::Instrument;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

fn main() -> ExitCode {
    // Baseline subscriber: stderr at WARN+ unless `RUST_LOG` overrides.
    // Installed before any span is created so the per-invocation span
    // is registered with an active subscriber (otherwise tracing would
    // treat it as disabled and correlation_id would not propagate to
    // downstream events). `try_init` is best-effort — a duplicate init
    // attempt from `cli::run` will simply no-op.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .try_init();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("gitlore: failed to start tokio runtime: {err}");
            // Exit 2 distinguishes process-bootstrap failures from
            // runtime errors surfaced by cli::run (which exit 1).
            return ExitCode::from(2);
        }
    };

    runtime.block_on(async {
        // UUIDv7: monotonic + time-sortable, unique per invocation.
        let correlation_id = Uuid::now_v7();
        let span = tracing::info_span!("gitlore", %correlation_id);

        match gitlore::cli::run().instrument(span).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                // cli::run is expected to have emitted a structured
                // tracing event on the failure path; this stderr line
                // is the final human-readable line on exit.
                eprintln!("gitlore: {err}");
                ExitCode::FAILURE
            }
        }
    })
}

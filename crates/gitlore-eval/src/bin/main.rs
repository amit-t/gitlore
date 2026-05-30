//! `gitlore-eval` CLI entrypoint.
//!
//! Usage:
//!
//! ```text
//! gitlore-eval scenarios <name>
//! ```
//!
//! Looks up `<name>` in the scenario registry and runs it. Exits non-zero
//! when the scenario is not found, errors during execution, or fails its
//! metric threshold.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use gitlore_eval::scenarios;

#[derive(Parser, Debug)]
#[command(
    name = "gitlore-eval",
    about = "Evaluation harness for gitlore (search / story / risk scenarios)",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a named scenario from the registry.
    Scenarios {
        /// Scenario name as registered in `gitlore_eval::scenarios`.
        name: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Scenarios { name } => match scenarios::run(&name) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

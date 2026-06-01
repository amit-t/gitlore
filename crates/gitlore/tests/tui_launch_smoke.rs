//! TUI launch smoke test for `gitlore` (AC-INIT-1).
//!
//! Goals (per fix_plan M1 entry):
//!
//! * boot `gitlore` inside a tempdir fixture git repo
//! * assert the process is alive within 1 second of launch
//! * assert it exits cleanly when `q` is pressed
//! * edge case: 20×10 terminal must not panic
//! * idempotent relaunch in the same repo
//!
//! ## Required dev-dependencies in `crates/gitlore/Cargo.toml`
//!
//! ```toml
//! [dev-dependencies]
//! assert_cmd = "2"
//! tempfile = "3"
//! portable-pty = "0.8"
//! ```
//!
//! `assert_cmd` resolves the freshly built `gitlore` binary path. A pty pair
//! is required because `crossterm` reads key events from the controlling tty
//! (not stdin); a piped stdin would never deliver a `q` keypress to ratatui.
//! `portable-pty` keeps the test working on both macOS and Linux runners.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tempfile::TempDir;

/// Wall-clock budget gitlore has to stay alive after launch (AC-INIT-1).
const ALIVE_WINDOW: Duration = Duration::from_secs(1);

/// Wall-clock budget gitlore has to exit after receiving `q`.
const QUIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Build a minimal one-commit git repo inside a fresh tempdir.
///
/// Uses the system `git` binary; gitlore itself is read-only against the
/// repo, so any valid git layout is sufficient for a launch smoke test.
fn make_fixture_repo() -> TempDir {
    let tmp = TempDir::new().expect("create tempdir");
    let path = tmp.path();

    run_git(path, &["init", "--quiet", "--initial-branch=main"]);
    run_git(path, &["config", "user.email", "test@gitlore.dev"]);
    run_git(path, &["config", "user.name", "gitlore-test"]);
    run_git(path, &["config", "commit.gpgsign", "false"]);

    std::fs::write(path.join("README.md"), "fixture\n").expect("write README");
    run_git(path, &["add", "README.md"]);
    run_git(
        path,
        &["commit", "--quiet", "-m", "fixture: initial commit"],
    );

    tmp
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        // Insulate from host git config (user, hooks, includes).
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} exited {status:?}");
}

/// Launch `gitlore` inside `repo` under a pty sized `cols × rows`, wait the
/// 1-second alive window, send `q`, then assert clean exit.
///
/// Returns the recorded exit status. Panics if gitlore dies before the
/// alive window expires or fails to exit within `QUIT_TIMEOUT`.
fn launch_and_quit(repo: &Path, cols: u16, rows: u16) -> portable_pty::ExitStatus {
    // Resolve the freshly built binary the way `cargo test` expects.
    let bin = StdCommand::cargo_bin("gitlore").expect("locate gitlore binary");
    let program = bin.get_program().to_os_string();

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("allocate pty");

    let mut cmd = CommandBuilder::new(&program);
    cmd.cwd(repo);
    // Give crossterm a sane TERM and isolate gitlore from host config.
    cmd.env("TERM", "xterm-256color");
    cmd.env("NO_COLOR", "1");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .expect("spawn gitlore under pty");
    // Drop slave so the master is the sole owner of the pty.
    drop(pair.slave);

    let mut writer = pair.master.take_writer().expect("take pty master writer");

    // Hold a reader handle to drain pty output; without it the slave can
    // block on a full pty buffer once ratatui starts drawing frames.
    let mut reader = pair.master.try_clone_reader().expect("clone pty reader");
    let drain = std::thread::spawn(move || {
        let mut sink = [0u8; 4096];
        // Read until EOF; ignore errors after the child exits.
        while let Ok(n) = reader.read(&mut sink) {
            if n == 0 {
                break;
            }
        }
    });

    // AC-INIT-1: process must still be running 1s after launch.
    let start = Instant::now();
    while start.elapsed() < ALIVE_WINDOW {
        if let Some(exit) = child.try_wait().expect("poll child") {
            panic!("gitlore exited within the {ALIVE_WINDOW:?} alive window with status {exit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Send `q`. Some terminals expect a CR/LF after a key; ratatui's
    // crossterm event loop should pick up the byte unbuffered, but flushing
    // is cheap insurance against pty write batching.
    writer.write_all(b"q").expect("write q");
    writer.flush().ok();

    // Wait for graceful exit.
    let deadline = Instant::now() + QUIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("poll child for exit") {
            // Allow the drain thread to finish; ignore join errors.
            drop(writer);
            let _ = drain.join();
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("gitlore did not exit within {QUIT_TIMEOUT:?} after `q` keypress");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// AC-INIT-1: boots inside a git repo, stays alive ≥1s, exits cleanly on `q`.
#[test]
fn boots_alive_and_quits_on_q() {
    let repo = make_fixture_repo();
    let status = launch_and_quit(repo.path(), 80, 24);
    assert!(
        status.success(),
        "gitlore exited non-zero on `q`: {status:?}"
    );
}

/// Edge case: minimum sensible terminal size must not panic.
///
/// 20×10 is far below any reasonable layout; ratatui must clamp gracefully.
#[test]
fn small_terminal_20x10_does_not_panic() {
    let repo = make_fixture_repo();
    let status = launch_and_quit(repo.path(), 20, 10);
    assert!(
        status.success(),
        "gitlore did not exit cleanly at 20x10: {status:?}"
    );
}

/// Idempotent relaunch: closing and reopening in the same repo behaves
/// identically. Catches bugs where first-run state leaks into the working
/// dir (lockfiles, stale pidfiles, half-written caches).
#[test]
fn idempotent_relaunch_in_same_repo() {
    let repo = make_fixture_repo();
    for attempt in 0..3 {
        let status = launch_and_quit(repo.path(), 80, 24);
        assert!(
            status.success(),
            "relaunch attempt {attempt} exited non-zero: {status:?}"
        );
    }
}

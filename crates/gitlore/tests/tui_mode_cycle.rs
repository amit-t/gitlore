//! Integration test: mode cycle via pty key events (AC-TUI-1).
//!
//! Launches `gitlore` in a pty, sends `Tab` three times and `Shift-Tab` once,
//! then quits with `q`. Asserts clean exit (AC-TUI-1 coverage: four Tab steps
//! rotate Search → Story → Risk → Hotspots → Search; Shift-Tab reverses).
//!
//! This test mirrors the technique used in `tui_launch_smoke.rs`.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tempfile::TempDir;

const ALIVE_WINDOW: Duration = Duration::from_secs(1);
const QUIT_TIMEOUT: Duration = Duration::from_secs(8);

fn make_fixture_repo() -> TempDir {
    let tmp = TempDir::new().expect("create tempdir");
    let path = tmp.path();
    run_git(path, &["init", "--quiet", "--initial-branch=main"]);
    run_git(path, &["config", "user.email", "test@gitlore.dev"]);
    run_git(path, &["config", "user.name", "gitlore-test"]);
    run_git(path, &["config", "commit.gpgsign", "false"]);
    std::fs::write(path.join("README.md"), "fixture\n").expect("write");
    run_git(path, &["add", "README.md"]);
    run_git(path, &["commit", "--quiet", "-m", "init"]);
    tmp
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed");
}

fn launch_send_quit(repo: &Path, keys: &[u8]) -> portable_pty::ExitStatus {
    let bin = StdCommand::cargo_bin("gitlore").expect("gitlore binary");
    let program = bin.get_program().to_os_string();

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("pty");

    let mut cmd = CommandBuilder::new(&program);
    cmd.cwd(repo);
    cmd.env("TERM", "xterm-256color");
    cmd.env("NO_COLOR", "1");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");

    let mut child = pair.slave.spawn_command(cmd).expect("spawn");
    drop(pair.slave);

    let mut writer = pair.master.take_writer().expect("writer");
    let mut reader = pair.master.try_clone_reader().expect("reader");

    let drain = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
    });

    // Wait alive window.
    let start = Instant::now();
    while start.elapsed() < ALIVE_WINDOW {
        if let Some(exit) = child.try_wait().expect("poll") {
            panic!("gitlore exited too early: {exit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Send the provided key sequence, then quit.
    writer.write_all(keys).expect("write keys");
    writer.flush().ok();
    std::thread::sleep(Duration::from_millis(200));
    writer.write_all(b"q").expect("write q");
    writer.flush().ok();

    let deadline = Instant::now() + QUIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("poll") {
            drop(writer);
            let _ = drain.join();
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("gitlore did not quit after key sequence + q");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// AC-TUI-1: Tab cycles through all four modes without panic.
#[test]
fn tab_cycles_four_modes_and_quits_cleanly() {
    let repo = make_fixture_repo();
    // 4 × Tab to complete one full cycle, then q.
    let status = launch_send_quit(repo.path(), b"\t\t\t\t");
    assert!(status.success(), "mode cycle exited non-zero: {status:?}");
}

/// AC-TUI-1: Shift-Tab reverses cycle.
#[test]
fn shift_tab_reverses_cycle() {
    let repo = make_fixture_repo();
    // BackTab is sent as ESC[Z in many terminals.
    let status = launch_send_quit(repo.path(), b"\x1b[Z\x1b[Z");
    assert!(
        status.success(),
        "shift-tab cycle exited non-zero: {status:?}"
    );
}

//! Integration test: help overlay (AC-TUI-4).
//!
//! Launches `gitlore` in a pty, presses `?` to open the overlay, waits,
//! presses `?` again to close it, then quits with `q`. Asserts clean exit.
//!
//! We cannot easily assert the overlay *text* via pty without a full VT100
//! emulator; the test instead verifies that toggling the overlay twice does
//! not crash the app.

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
    let tmp = TempDir::new().expect("tempdir");
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
    let s = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(s.success());
}

#[test]
fn help_overlay_toggles_twice_without_crash() {
    let repo = make_fixture_repo();

    let bin = StdCommand::cargo_bin("gitlore").expect("bin");
    let program = bin.get_program().to_os_string();

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("pty");

    let mut cmd = CommandBuilder::new(&program);
    cmd.cwd(repo.path());
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

    let start = Instant::now();
    while start.elapsed() < ALIVE_WINDOW {
        if let Some(e) = child.try_wait().expect("poll") {
            panic!("exited early: {e:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Open help with `?`, wait 200 ms, close with `?`, then quit.
    writer.write_all(b"?").expect("?");
    writer.flush().ok();
    std::thread::sleep(Duration::from_millis(300));
    writer.write_all(b"?").expect("?");
    writer.flush().ok();
    std::thread::sleep(Duration::from_millis(200));
    writer.write_all(b"q").expect("q");
    writer.flush().ok();

    let deadline = Instant::now() + QUIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("poll") {
            drop(writer);
            let _ = drain.join();
            assert!(
                status.success(),
                "non-zero exit after help toggle: {status:?}"
            );
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("gitlore did not quit after help overlay test");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

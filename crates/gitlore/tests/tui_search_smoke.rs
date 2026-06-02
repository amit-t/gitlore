//! TUI search-mode smoke test (AC-TUI-2).
//!
//! Setup:
//! 1. Create a temp git repo with a few commits.
//! 2. Run `gitlore index` via assert_cmd to build the SQLite index.
//! 3. Launch the TUI in a pty.
//! 4. Press `/` to focus the search bar (Nav → Input mode).
//! 5. Type "fix" + Enter to submit a query.
//! 6. Wait 1 s for the search result to render.
//! 7. Quit with `q` (which re-enters Nav mode via Escape first if needed).
//!
//! We only assert clean exit; diff pane content and result count are not
//! observable via raw pty without a VT100 parser.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tempfile::TempDir;

const ALIVE_WINDOW: Duration = Duration::from_secs(1);
const QUIT_TIMEOUT: Duration = Duration::from_secs(10);

fn make_indexed_fixture_repo() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path();

    run_git(path, &["init", "--quiet", "--initial-branch=main"]);
    run_git(path, &["config", "user.email", "test@gitlore.dev"]);
    run_git(path, &["config", "user.name", "gitlore-test"]);
    run_git(path, &["config", "commit.gpgsign", "false"]);

    for i in 0..5 {
        let name = format!("file{i}.txt");
        std::fs::write(path.join(&name), format!("content {i}\n")).expect("write");
        run_git(path, &["add", "."]);
        run_git(
            path,
            &[
                "commit",
                "--quiet",
                "-m",
                &format!("fix: change {name} for issue {i}"),
            ],
        );
    }

    // Run `gitlore index` so the SQLite DB exists.
    let index_status = StdCommand::cargo_bin("gitlore")
        .expect("gitlore bin")
        .arg("index")
        .current_dir(path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("NO_COLOR", "1")
        .status()
        .expect("run gitlore index");
    assert!(
        index_status.success(),
        "gitlore index failed: {index_status:?}"
    );

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
    assert!(s.success(), "git {args:?} failed");
}

/// AC-TUI-2: TUI launches, accepts a search query via `/`, submits, and exits
/// cleanly after `Esc` (return to Nav) then `q`.
#[test]
fn search_mode_accepts_query_and_quits_cleanly() {
    let repo = make_indexed_fixture_repo();

    let bin = StdCommand::cargo_bin("gitlore").expect("bin");
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
        if let Some(exit) = child.try_wait().expect("poll") {
            panic!("gitlore exited early: {exit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // `/` → focus search (Input mode).
    writer.write_all(b"/").expect("write /");
    writer.flush().ok();
    std::thread::sleep(Duration::from_millis(100));

    // Type "fix" + Enter → submit.
    writer.write_all(b"fix\r").expect("write query + Enter");
    writer.flush().ok();
    std::thread::sleep(Duration::from_millis(500));

    // Esc → back to Nav (in case we're still in Input mode).
    writer.write_all(b"\x1b").expect("write Esc");
    writer.flush().ok();
    std::thread::sleep(Duration::from_millis(200));

    // `q` → quit.
    writer.write_all(b"q").expect("write q");
    writer.flush().ok();

    let deadline = Instant::now() + QUIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("poll") {
            drop(writer);
            let _ = drain.join();
            assert!(status.success(), "search smoke exited non-zero: {status:?}");
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("gitlore did not quit after search smoke test");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

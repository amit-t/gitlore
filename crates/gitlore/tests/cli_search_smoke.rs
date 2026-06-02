//! Smoke test for `gitlore search` (M4 / TDD-001 §4.1).
//!
//! Builds a tiny repo with a commit whose message contains "retry", runs
//! `gitlore index` to populate the SQLite index, then runs
//! `gitlore search "retry"` and asserts:
//!
//! * exit code 0
//! * stdout is non-empty
//! * stderr does not contain a Rust panic backtrace

#![allow(clippy::needless_pass_by_value)]

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn build_fixture_repo() -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("gitlore-search-smoke-")
        .tempdir()
        .expect("tempdir");
    let root = dir.path();
    run_git(root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(root, &["config", "user.email", "search-smoke@gitlore.dev"]);
    run_git(root, &["config", "user.name", "search-smoke"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);
    write_file(root, "README.md", "# fixture\n");
    write_file(root, "src/client.rs", "pub fn connect() {}\n");
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "feat: initial"]);
    write_file(
        root,
        "src/client.rs",
        "pub fn connect() {}\npub fn retry() {}\n",
    );
    run_git(root, &["add", "src/client.rs"]);
    run_git(root, &["commit", "--quiet", "-m", "fix: retry on timeout"]);
    dir
}

fn write_file(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, contents).unwrap();
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed in {}", cwd.display());
}

fn run_gitlore(repo: &Path, args: &[&str]) -> std::process::Output {
    let bin = cargo_bin("gitlore");
    Command::new(&bin)
        .current_dir(repo)
        .args(args)
        .env(
            "XDG_DATA_HOME",
            repo.parent().unwrap_or(repo).join("xdg-data"),
        )
        .env(
            "XDG_CONFIG_HOME",
            repo.parent().unwrap_or(repo).join("xdg-config"),
        )
        .env(
            "XDG_CACHE_HOME",
            repo.parent().unwrap_or(repo).join("xdg-cache"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("spawn gitlore {args:?}: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn search_exits_zero_and_produces_output() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();

    let idx_out = run_gitlore(repo, &["index"]);
    assert!(
        idx_out.status.success(),
        "index failed (exit={:?})\nstdout={}\nstderr={}",
        idx_out.status.code(),
        String::from_utf8_lossy(&idx_out.stdout),
        String::from_utf8_lossy(&idx_out.stderr),
    );

    let out = run_gitlore(repo, &["search", "retry"]);

    assert!(
        out.status.success(),
        "gitlore search exited non-zero (code={:?})\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "expected non-empty stdout from `gitlore search retry`, got nothing"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked at"),
        "gitlore search produced a panic on stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("stack backtrace:"),
        "gitlore search produced a stack backtrace on stderr:\n{stderr}"
    );
}

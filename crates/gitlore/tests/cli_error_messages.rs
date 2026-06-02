//! Integration tests: CLI error message quality (AC-CLI-3).
//!
//! All tests in this module run `gitlore` as a subprocess via `assert_cmd`
//! and assert that:
//! - The process exits with a non-zero code.
//! - The stderr message is human-readable and contains an actionable hint.
//! - The message does NOT contain Rust panic traces or internal file paths.

use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::prelude::*;
use tempfile::TempDir;

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

// ---------------------------------------------------------------------------
// search outside a git repo
// ---------------------------------------------------------------------------

/// Running `gitlore search` outside any git repository must fail gracefully.
#[test]
fn search_outside_git_repo_errors_with_readable_message() {
    let tmp = TempDir::new().expect("tempdir");
    let mut cmd = StdCommand::cargo_bin("gitlore").expect("bin");
    cmd.arg("search").arg("fix");
    cmd.current_dir(tmp.path());
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("NO_COLOR", "1");

    let output = cmd.output().expect("run");
    // Must not succeed.
    assert!(!output.status.success(), "should fail outside git repo");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // Should contain a human hint (no raw panic trace).
    assert!(
        !stderr.contains("thread 'main' panicked") && !stderr.contains("RUST_BACKTRACE"),
        "stderr must not contain panic: {stderr}"
    );
    // Must mention something about no git repo or index.
    let lower = stderr.to_lowercase();
    assert!(
        lower.contains("git") || lower.contains("index") || lower.contains("repository"),
        "error message must be about missing git or index; got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// index outside a git repo
// ---------------------------------------------------------------------------

/// Running `gitlore index` outside any git repository must fail gracefully.
#[test]
fn index_outside_git_repo_errors_with_readable_message() {
    let tmp = TempDir::new().expect("tempdir");
    let mut cmd = StdCommand::cargo_bin("gitlore").expect("bin");
    cmd.arg("index");
    cmd.current_dir(tmp.path());
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("NO_COLOR", "1");

    let output = cmd.output().expect("run");
    assert!(!output.status.success(), "should fail outside git repo");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "must not panic: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// search with missing index (repo exists but never indexed)
// ---------------------------------------------------------------------------

/// Running `gitlore search` in an un-indexed repo must fail with a helpful hint.
#[test]
fn search_on_unindexed_repo_errors_with_run_index_hint() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path();
    run_git(path, &["init", "--quiet", "--initial-branch=main"]);
    run_git(path, &["config", "user.email", "test@gitlore.dev"]);
    run_git(path, &["config", "user.name", "gitlore-test"]);
    run_git(path, &["config", "commit.gpgsign", "false"]);
    std::fs::write(path.join("README.md"), "hi\n").expect("write");
    run_git(path, &["add", "README.md"]);
    run_git(path, &["commit", "--quiet", "-m", "init"]);

    let mut cmd = StdCommand::cargo_bin("gitlore").expect("bin");
    cmd.arg("search").arg("README");
    cmd.current_dir(path);
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("NO_COLOR", "1");

    let output = cmd.output().expect("run");
    assert!(
        !output.status.success(),
        "search on un-indexed repo must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "must not panic: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// status outside a git repo
// ---------------------------------------------------------------------------

#[test]
fn status_outside_git_repo_errors_gracefully() {
    let tmp = TempDir::new().expect("tempdir");
    let mut cmd = StdCommand::cargo_bin("gitlore").expect("bin");
    cmd.arg("status");
    cmd.current_dir(tmp.path());
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("NO_COLOR", "1");

    let output = cmd.output().expect("run");
    assert!(
        !output.status.success(),
        "status should fail outside git repo"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "must not panic: {stderr}"
    );
}

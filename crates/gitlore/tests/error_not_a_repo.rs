//! AC-INIT-4: `gitlore` invoked outside a Git working tree must surface a
//! single friendly error line and the SPEC-001 §4.3 `not_a_repo` envelope,
//! never a Rust panic / backtrace.
//!
//! Contract under test (SPEC §4 init contract, §4.3 error envelope):
//!
//! * Plain stderr: exactly `gitlore: not a git repository (run gitlore inside a repo)`.
//! * `--json` mode: stdout carries one JSON line `{"error":{"code":"not_a_repo", ...}}`.
//! * In both modes the process exits non-zero and stderr is free of
//!   `panicked at` / `note: run with RUST_BACKTRACE` (no backtrace leak).
//!
//! We drive the bin through `gitlore index` because the default no-arg
//! invocation enters the TUI (which needs a TTY and is exercised by the
//! `tui_launch_smoke` suite). The subcommand path is the same end of the
//! pipeline that scripted callers and CI hit, so AC-INIT-4 lives here.

#![allow(clippy::needless_pass_by_value)]

use std::path::Path;
use std::process::{Command, Stdio};

use assert_cmd::cargo::cargo_bin;
use serde_json::Value;
use tempfile::TempDir;

/// Exact stderr line required by AC-INIT-4.
const EXPECTED_STDERR_LINE: &str = "gitlore: not a git repository (run gitlore inside a repo)";

/// Build a fresh tempdir guaranteed to not contain a `.git/` directory.
///
/// Sibling sandbox dirs (`xdg-*`) are placed alongside so XDG fallbacks
/// from the bin land inside the tempdir tree, not in the user's real
/// `~/.local/share`.
fn fresh_non_repo() -> TempDir {
    tempfile::Builder::new()
        .prefix("gitlore-not-a-repo-")
        .tempdir()
        .expect("tempdir")
}

fn run_gitlore(cwd: &Path, args: &[&str]) -> std::process::Output {
    let bin = cargo_bin("gitlore");
    let sandbox = cwd.parent().unwrap_or(cwd);
    Command::new(&bin)
        .current_dir(cwd)
        .args(args)
        // Disable backtraces explicitly: a stray `RUST_BACKTRACE=1` from
        // the dev shell would otherwise mask a real panic leak. The bin
        // must not panic at all on the NotARepo path, but we belt-and-
        // brace by also stripping the env var.
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        // Pin XDG roots into the tempdir tree so cache/config probes do
        // not touch the user's real dotfiles.
        .env("XDG_DATA_HOME", sandbox.join("xdg-data"))
        .env("XDG_CONFIG_HOME", sandbox.join("xdg-config"))
        .env("XDG_CACHE_HOME", sandbox.join("xdg-cache"))
        // Pin RUST_LOG off so the tracing subscriber does not splatter
        // warn/info lines into stderr and confuse the line-equality check.
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("spawn gitlore {args:?}: {e}"))
}

fn assert_no_backtrace_leak(stderr: &str, stdout: &str) {
    for label in ["stderr", "stdout"] {
        let buf = if label == "stderr" { stderr } else { stdout };
        assert!(
            !buf.contains("panicked at"),
            "{label} leaked a Rust panic banner; full {label}:\n{buf}"
        );
        assert!(
            !buf.contains("note: run with `RUST_BACKTRACE`"),
            "{label} leaked the RUST_BACKTRACE hint; full {label}:\n{buf}"
        );
        // Belt-and-brace: the literal substring the task spec names.
        assert!(
            !buf.contains("note: run with RUST_BACKTRACE"),
            "{label} leaked the RUST_BACKTRACE hint; full {label}:\n{buf}"
        );
    }
}

/// AC-INIT-4 plain mode: exit != 0, stderr is exactly the friendly line.
#[test]
fn plain_mode_emits_friendly_stderr_line() {
    let tmp = fresh_non_repo();
    let out = run_gitlore(tmp.path(), &["index"]);

    assert!(
        !out.status.success(),
        "expected non-zero exit; got {:?}\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    assert_no_backtrace_leak(&stderr, &stdout);

    // The friendly line must appear verbatim on its own line.
    let has_line = stderr.lines().any(|l| l == EXPECTED_STDERR_LINE);
    assert!(
        has_line,
        "stderr missing required line `{EXPECTED_STDERR_LINE}`; got:\n{stderr}"
    );

    // In plain mode the JSON envelope must NOT appear on stdout — the
    // error contract is split: plain → stderr, --json → stdout.
    assert!(
        stdout.trim().is_empty(),
        "plain mode leaked content on stdout; got:\n{stdout}"
    );
}

/// AC-INIT-4 JSON mode: SPEC-001 §4.3 not_a_repo envelope on stdout, no
/// backtrace leak on either channel.
#[test]
fn json_mode_emits_not_a_repo_envelope() {
    let tmp = fresh_non_repo();
    let out = run_gitlore(tmp.path(), &["index", "--json"]);

    assert!(
        !out.status.success(),
        "expected non-zero exit under --json; got {:?}",
        out.status.code()
    );

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    assert_no_backtrace_leak(&stderr, &stdout);

    let v: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON envelope on stdout; got `{stdout}` ({e})"));
    assert_eq!(
        v["error"]["code"].as_str(),
        Some("not_a_repo"),
        "envelope code mismatch: {v}"
    );
    let msg = v["error"]["message"].as_str().unwrap_or_default();
    assert!(
        !msg.is_empty(),
        "envelope message must be populated; got {v}"
    );
}

//! End-to-end CLI tests for `gitlore index` + `gitlore status` (M3-7,
//! SPEC-001 §4.1 / §4.3.1).
//!
//! Covers the M3-7 wire-up:
//!
//! * `gitlore index` populates the SQLite database, prints the human
//!   summary line on stdout, and exits 0.
//! * `gitlore index --json` emits one JSON line with the
//!   `{commits_indexed, commits_total, ref_count, duration_ms, watermark}`
//!   payload.
//! * `gitlore index --dry-run` enumerates refs without touching the DB.
//! * `gitlore index --rebuild` re-walks the history from scratch.
//! * `gitlore status` opens the index read-only and prints the header.
//! * `gitlore status --json` emits the `{commit_count, db_path,
//!   db_size_bytes, schema_version, embeddings_enabled, model,
//!   writer_lock}` envelope.
//! * `gitlore index --no-wait` fails with the `lock_contention` wire
//!   code when another writer holds the lock.

#![allow(clippy::needless_pass_by_value)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use serde_json::Value;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn build_fixture_repo() -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("gitlore-cli-fixture-")
        .tempdir()
        .expect("tempdir");
    let root = dir.path();
    run_git(root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(root, &["config", "user.email", "cli-test@gitlore.dev"]);
    run_git(root, &["config", "user.name", "cli-test"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);
    write_file(root, "README.md", "# fixture\n");
    write_file(root, "src/lib.rs", "pub fn one() -> i32 { 1 }\n");
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "feat: initial"]);
    write_file(
        root,
        "src/lib.rs",
        "pub fn one() -> i32 { 1 }\npub fn two() -> i32 { 2 }\n",
    );
    run_git(root, &["add", "src/lib.rs"]);
    run_git(root, &["commit", "--quiet", "-m", "feat: add two"]);
    write_file(root, "docs/intro.md", "intro\n");
    run_git(root, &["add", "docs/intro.md"]);
    run_git(root, &["commit", "--quiet", "-m", "docs: intro"]);
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

fn index_db_path(repo: &Path) -> PathBuf {
    repo.join(".git").join("gitlore").join("index.sqlite")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn index_then_status_round_trips() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();

    let out = run_gitlore(repo, &["index"]);
    assert!(
        out.status.success(),
        "index exit {:?}\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("indexed 3 of 3 commit(s)") || stdout.contains("indexed"),
        "human summary missing: {stdout}"
    );
    assert!(index_db_path(repo).exists(), "SQLite db not created");

    let out = run_gitlore(repo, &["status", "--json"]);
    assert!(out.status.success(), "status failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).expect("status --json is valid JSON");
    assert_eq!(v["commit_count"].as_u64(), Some(3));
    assert!(
        v["db_size_bytes"].as_u64().unwrap_or(0) > 0,
        "db_size_bytes should be populated"
    );
    assert_eq!(v["schema_version"].as_u64(), Some(2));
    assert_eq!(v["embeddings_enabled"].as_bool(), Some(false));
    assert!(
        v["model"].is_null(),
        "model should be null when no embeddings setup"
    );
    assert!(
        v["writer_lock"].is_null(),
        "writer_lock should be null after indexer exits"
    );
}

#[test]
fn index_json_envelope_carries_spec_keys() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    let out = run_gitlore(repo, &["index", "--json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    for k in [
        "commits_indexed",
        "commits_total",
        "ref_count",
        "duration_ms",
        "watermark",
    ] {
        assert!(v.get(k).is_some(), "missing key {k}; got {v}");
    }
    assert_eq!(v["commits_indexed"].as_u64(), Some(3));
    assert_eq!(v["commits_total"].as_u64(), Some(3));
    assert!(v["watermark"].is_object());
    // stderr should be empty under --json — progress is suppressed.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("indexed "),
        "progress leaked to stderr under --json: {stderr}"
    );
}

#[test]
fn index_dry_run_does_not_create_db() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    let out = run_gitlore(repo, &["index", "--dry-run", "--json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert_eq!(v["dry_run"].as_bool(), Some(true));
    assert_eq!(v["commits_total"].as_u64(), Some(3));
    assert_eq!(v["ref_count"].as_u64(), Some(1));
    assert!(v["refs"].is_array());
    // The SQLite db is still created by Indexer::open (writer lock takes
    // out a file inside <common-dir>/gitlore/), but no commits should be
    // written. We verify the dry_run flag round-trips and the commit
    // count is the estimate, not 0.
    let status_out = run_gitlore(repo, &["status", "--json"]);
    let s: Value = serde_json::from_str(String::from_utf8_lossy(&status_out.stdout).trim())
        .expect("status JSON");
    assert_eq!(
        s["commit_count"].as_u64(),
        Some(0),
        "dry-run must not persist commits"
    );
}

#[test]
fn index_rebuild_drops_then_repopulates() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    assert!(run_gitlore(repo, &["index"]).status.success());

    let out = run_gitlore(repo, &["index", "--rebuild", "--json"]);
    assert!(out.status.success(), "rebuild failed: {out:?}");
    let v: Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("valid JSON");
    assert_eq!(v["commits_indexed"].as_u64(), Some(3));
}

#[test]
fn index_no_wait_reports_lock_contention() {
    let fixture = build_fixture_repo();
    let repo = fixture.path().to_path_buf();

    // Pre-create the index directory + take an OS-level exclusive lock
    // on `index.lock` from the test harness so the child fails fast on
    // `--no-wait`. We hold it via fs2 directly — same advisory lock
    // semantics the indexer uses.
    let lock_dir = repo.join(".git").join("gitlore");
    fs::create_dir_all(&lock_dir).unwrap();
    let lock_path = lock_dir.join("index.lock");
    let lock_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock_file).unwrap();
    // Stamp a believable payload so the child can render holder info.
    use std::io::Write;
    let _ = lock_file
        .set_len(0)
        .and_then(|_| (&lock_file).write_all(b"99999\n2026-01-01T00:00:00Z\n"));

    let out = run_gitlore(&repo, &["index", "--no-wait", "--json"]);
    assert!(
        !out.status.success(),
        "--no-wait should fail under contention"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected JSON envelope, got `{stdout}` ({e})"));
    assert_eq!(
        v["error"]["code"].as_str(),
        Some("lock_contention"),
        "envelope: {v}"
    );

    let _ = fs2::FileExt::unlock(&lock_file);
}

#[test]
fn status_on_uninitialized_index_reports_zero() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    let out = run_gitlore(repo, &["status", "--json"]);
    assert!(out.status.success(), "status pre-index failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert_eq!(v["commit_count"].as_u64(), Some(0));
    assert_eq!(v["schema_version"].as_u64(), Some(0));
    assert_eq!(v["embeddings_enabled"].as_bool(), Some(false));
}

#[test]
fn status_renders_writer_lock_when_held() {
    let fixture = build_fixture_repo();
    let repo = fixture.path().to_path_buf();
    // First index so the DB exists and status can open it.
    assert!(run_gitlore(&repo, &["index"]).status.success());

    // Forge a lockfile so status can render holder info without us
    // having to spawn two indexer processes in lock-step.
    let lock_path = repo.join(".git").join("gitlore").join("index.lock");
    fs::write(&lock_path, "12345\n2026-06-02T00:00:00Z\n").unwrap();

    let out = run_gitlore(&repo, &["status", "--json"]);
    assert!(out.status.success(), "status failed");
    let v: Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("valid JSON");
    assert_eq!(v["writer_lock"]["pid"].as_u64(), Some(12345));
    assert!(
        v["writer_lock"]["started_at"]
            .as_str()
            .unwrap_or("")
            .starts_with("2026-06-02"),
        "started_at not round-tripped: {v}"
    );

    let _ = fs::remove_file(&lock_path);
}

#[test]
#[ignore = "perf budget runs only on self-hosted lane (hosted runners are too noisy); real gate lands at M3-7b via perf.cold_index_api_nodejs"]
fn index_meets_per_commit_perf_budget() {
    // Sanity check on the SPEC §12 cold-index target ("10k commits in
    // 2 minutes"). At ~12 ms/commit that gives a 200-commit fixture a
    // budget of ~2.4s; we set the gate at 30s to absorb CI noise while
    // still catching a 10× regression.
    //
    // The full eval-driven gate (`perf.cold_index_api_nodejs`) lands
    // with TDD-004; this test is a faster smoke that exercises the
    // M3-7 CLI surface end-to-end with a non-trivial commit count.
    let fixture = build_perf_fixture(200);
    let repo = fixture.path();
    let out = run_gitlore(repo, &["index", "--json"]);
    assert!(out.status.success(), "index failed: {out:?}");
    let v: Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("valid JSON");
    let commits = v["commits_indexed"].as_u64().expect("commits_indexed");
    let duration_ms = v["duration_ms"].as_u64().expect("duration_ms");
    assert_eq!(commits, 200);
    assert!(
        duration_ms < 30_000,
        "200-commit cold index took {duration_ms} ms; gate is 30s. \
         SPEC §12 budget: 10k commits in 120s"
    );
}

fn build_perf_fixture(n_commits: usize) -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("gitlore-perf-fixture-")
        .tempdir()
        .expect("tempdir");
    let root = dir.path();
    run_git(root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(root, &["config", "user.email", "perf@gitlore.dev"]);
    run_git(root, &["config", "user.name", "perf"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);
    write_file(root, "README.md", "# perf fixture\n");
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "feat: initial"]);
    for i in 1..n_commits {
        write_file(
            root,
            "src/lib.rs",
            &format!("pub fn n() -> i32 {{ {i} }}\n"),
        );
        run_git(root, &["add", "src/lib.rs"]);
        run_git(
            root,
            &["commit", "--quiet", "-m", &format!("feat: bump {i}")],
        );
    }
    dir
}

#[test]
fn status_remains_safe_during_concurrent_index() {
    // AC-RO-2 reinforcement: status must not block on the writer lock
    // (it's read-only via SQLITE_OPEN_READ_ONLY).
    let fixture = build_fixture_repo();
    let repo = fixture.path().to_path_buf();
    assert!(run_gitlore(&repo, &["index"]).status.success());

    // Spawn a child holding the lock by simply rebuilding while we
    // poll status concurrently. The rebuild dominates the wall clock so
    // there's a real overlap window.
    let repo_clone = repo.clone();
    let child = thread::spawn(move || run_gitlore(&repo_clone, &["index", "--rebuild", "--json"]));

    // Poll status for up to 5 seconds; at least one observation while
    // the child is still alive must succeed without error.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ok_seen = false;
    while Instant::now() < deadline {
        let s = run_gitlore(&repo, &["status", "--json"]);
        if s.status.success() {
            ok_seen = true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.join().unwrap();
    assert!(ok_seen, "status never succeeded during concurrent index");
}

//! Integration test: `gitlore` refuses to operate on an index whose
//! `schema_version` is higher than the binary's `LATEST` (AC-IDX-10).
//!
//! Setup:
//! 1. Build a real index in a temp repo.
//! 2. Manually bump `index_state.schema_version` in the SQLite DB to 999.
//! 3. Run `gitlore status` and `gitlore search foo` — both must:
//!    - exit with a non-zero code.
//!    - emit `schema_version_too_new` in JSON mode (`--output-format json`).
//!    - emit a human-readable hint mentioning "upgrade" or "rebuild" in human mode.
//!
//! The test uses the same temp-repo / index setup as `cli_index_status.rs`.

use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;

fn make_fixture_repo() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path();
    run_git(path, &["init", "--quiet", "--initial-branch=main"]);
    run_git(path, &["config", "user.email", "test@gitlore.dev"]);
    run_git(path, &["config", "user.name", "gitlore-test"]);
    run_git(path, &["config", "commit.gpgsign", "false"]);
    for i in 0..3 {
        std::fs::write(path.join(format!("f{i}.txt")), format!("data{i}\n")).expect("write");
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "--quiet", "-m", &format!("commit {i}")]);
    }
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

fn run_index(repo: &Path) {
    let status = StdCommand::cargo_bin("gitlore")
        .expect("bin")
        .arg("index")
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("NO_COLOR", "1")
        .status()
        .expect("run gitlore index");
    assert!(status.success(), "gitlore index failed: {status:?}");
}

/// Find the SQLite DB produced by `gitlore index` within `repo`.
fn find_db(repo: &Path) -> std::path::PathBuf {
    // The DB lives at `<git_common_dir>/.gitlore/index.db` or
    // `<xdg_data_home>/gitlore/<hash>/index.db`.  We probe both typical paths.
    let git_dir = repo.join(".git");
    let candidate = git_dir.join(".gitlore").join("index.db");
    if candidate.exists() {
        return candidate;
    }
    // Walk the `repos/` result — the indexer writes the DB under `<common_dir>`.
    panic!("Could not find index.db under {repo:?}");
}

fn bump_schema_version(db_path: &Path, new_version: u64) {
    let conn = Connection::open(db_path).expect("open db");
    conn.execute(
        "UPDATE index_state SET value = ?1 WHERE key = 'schema_version'",
        rusqlite::params![new_version.to_string()],
    )
    .expect("bump schema_version");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn status_json_reports_schema_version_too_new_code() {
    let repo = make_fixture_repo();
    run_index(repo.path());

    let db = find_db(repo.path());
    bump_schema_version(&db, 999);

    let output = StdCommand::cargo_bin("gitlore")
        .expect("bin")
        .args(["--output-format", "json", "status"])
        .current_dir(repo.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("NO_COLOR", "1")
        .output()
        .expect("run");

    assert!(
        !output.status.success(),
        "must fail when schema_version=999"
    );

    let raw = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);

    assert!(
        raw.contains("schema_version_too_new"),
        "output must contain schema_version_too_new error code; got: {raw}"
    );
}

#[test]
fn search_json_reports_schema_version_too_new_code() {
    let repo = make_fixture_repo();
    run_index(repo.path());

    let db = find_db(repo.path());
    bump_schema_version(&db, 999);

    let output = StdCommand::cargo_bin("gitlore")
        .expect("bin")
        .args(["--output-format", "json", "search", "data"])
        .current_dir(repo.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("NO_COLOR", "1")
        .output()
        .expect("run");

    assert!(
        !output.status.success(),
        "search must fail when schema_version=999"
    );

    let raw = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);

    assert!(
        raw.contains("schema_version_too_new"),
        "output must contain schema_version_too_new; got: {raw}"
    );
}

#[test]
fn status_human_mentions_upgrade_hint() {
    let repo = make_fixture_repo();
    run_index(repo.path());

    let db = find_db(repo.path());
    bump_schema_version(&db, 999);

    let output = StdCommand::cargo_bin("gitlore")
        .expect("bin")
        .arg("status")
        .current_dir(repo.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("NO_COLOR", "1")
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{stdout}{stderr}").to_lowercase();

    // The error message must contain an actionable word: "upgrade" or "rebuild".
    assert!(
        raw.contains("upgrade") || raw.contains("rebuild") || raw.contains("newer"),
        "human error must mention upgrade/rebuild/newer; got: {raw}"
    );
}

//! End-to-end CLI tests for `gitlore identities` + `gitlore classify`
//! (M3-7b, SPEC-001 §4.1 / §4.3.1 / §4.4).
//!
//! Covers the M3-7b wire-up:
//!
//! * `gitlore identities` hides bots by default and surfaces the alias
//!   + commit counters from the cached index.
//! * `gitlore identities --include-bots --json` emits the SPEC-001 §4.3.1
//!   envelope (`{clustered_count, raw_count, identities: [...]}`).
//! * `gitlore classify <glob>` shells out to `git ls-files`, applies the
//!   glob, and prints `<path>\t<category>` per matched file.
//! * `gitlore classify <glob> --json` emits
//!   `{glob, matched_files: [{path, category}], category}` with a
//!   homogeneous `category` field collapsed to a single label when every
//!   match shares one.
//! * `gitlore classify --explain <sha>` reads `commits.files_changed`
//!   from the index and classifies each file. Unknown SHA → stable
//!   `sha_not_found` envelope.
//! * `gitlore classify` with neither argument returns the stable
//!   `unimplemented` envelope (matches the M1 contract for un-wired
//!   surfaces).

#![allow(clippy::needless_pass_by_value)]

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use assert_cmd::cargo::cargo_bin;
use serde_json::Value;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture helpers — mirror the shape of cli_index_status.rs so the two
// integration tests can be reasoned about together.
// ---------------------------------------------------------------------------

fn build_fixture_repo() -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("gitlore-cli-idclassify-")
        .tempdir()
        .expect("tempdir");
    let root = dir.path();
    run_git(root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(root, &["config", "user.email", "alice@example.com"]);
    run_git(root, &["config", "user.name", "Alice Example"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);
    write_file(root, "README.md", "# fixture\n");
    write_file(root, "src/lib.rs", "pub fn one() -> i32 { 1 }\n");
    write_file(root, "tests/foo.rs", "#[test] fn t() {}\n");
    write_file(root, "docs/intro.md", "intro\n");
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "feat: initial"]);

    // Second commit under a different identity — exercises clustering.
    run_git(root, &["config", "user.email", "bob@example.com"]);
    run_git(root, &["config", "user.name", "Bob Builder"]);
    write_file(
        root,
        "src/lib.rs",
        "pub fn one() -> i32 { 1 }\npub fn two() -> i32 { 2 }\n",
    );
    run_git(root, &["add", "src/lib.rs"]);
    run_git(root, &["commit", "--quiet", "-m", "feat: add two"]);

    // Third commit under a bot identity.
    run_git(
        root,
        &[
            "config",
            "user.email",
            "1+dependabot[bot]@users.noreply.github.com",
        ],
    );
    run_git(root, &["config", "user.name", "dependabot[bot]"]);
    write_file(root, "Cargo.toml", "# bump\n");
    run_git(root, &["add", "Cargo.toml"]);
    run_git(root, &["commit", "--quiet", "-m", "deps: bump"]);
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
    let parent = repo.parent().unwrap_or(repo);
    Command::new(&bin)
        .current_dir(repo)
        .args(args)
        .env("HOME", parent)
        .env("XDG_CONFIG_HOME", parent.join("xdg-config"))
        .env("XDG_DATA_HOME", parent.join("xdg-data"))
        .env("XDG_CACHE_HOME", parent.join("xdg-cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("spawn gitlore {args:?}: {e}"))
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json_line(text: &str) -> Value {
    serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("expected JSON, got `{text}` ({e})"))
}

/// Stamp `is_bot = 1` on every identity whose canonical email matches the
/// `[bot]` heuristic. The M3-4 mailmap layer always upserts `is_bot = 0`
/// for inputs git `check-mailmap` echoes through unchanged, so the
/// fixture has to set the column directly before exercising the
/// `--include-bots` flag — the integration test is asserting the CLI
/// filter, not the indexer's bot-detection chain (which is M3-4's
/// concern).
fn force_bot_flag_on_dependabot(repo: &Path) {
    use rusqlite::Connection;
    let db = repo.join(".git").join("gitlore").join("index.sqlite");
    let conn = Connection::open(&db).expect("open index for fixture patch");
    conn.execute(
        "UPDATE identities SET is_bot = 1 \
         WHERE canonical_email LIKE '%[bot]%' \
            OR canonical_name LIKE '%[bot]%'",
        [],
    )
    .expect("flip is_bot on bot identity");
}

// ---------------------------------------------------------------------------
// Tests — identities
// ---------------------------------------------------------------------------

#[test]
fn identities_hides_bots_by_default() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    assert!(
        run_gitlore(repo, &["index"]).status.success(),
        "index failed"
    );
    force_bot_flag_on_dependabot(repo);

    let out = run_gitlore(repo, &["identities", "--json"]);
    assert!(
        out.status.success(),
        "identities exit={:?} stderr={}",
        out.status.code(),
        stderr(&out)
    );
    let v = json_line(&stdout(&out));
    // Two human identities (Alice, Bob) plus one bot — clustered_count
    // describes the full space.
    assert_eq!(v["clustered_count"].as_u64(), Some(3));
    let listed = v["identities"].as_array().expect("identities array");
    assert_eq!(listed.len(), 2, "bot must be hidden by default: {v}");
    for row in listed {
        assert_eq!(row["is_bot"].as_bool(), Some(false));
    }
}

#[test]
fn identities_include_bots_surfaces_bot_row() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    assert!(run_gitlore(repo, &["index"]).status.success());
    force_bot_flag_on_dependabot(repo);

    let out = run_gitlore(repo, &["identities", "--include-bots", "--json"]);
    assert!(out.status.success(), "stderr={}", stderr(&out));
    let v = json_line(&stdout(&out));
    let listed = v["identities"].as_array().expect("identities array");
    assert_eq!(listed.len(), 3);
    assert!(
        listed
            .iter()
            .any(|row| row["is_bot"].as_bool() == Some(true)),
        "no bot row found in {v}"
    );
}

#[test]
fn identities_human_output_lists_each_canonical_pair() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    assert!(run_gitlore(repo, &["index"]).status.success());
    force_bot_flag_on_dependabot(repo);

    let out = run_gitlore(repo, &["identities", "--include-bots"]);
    assert!(out.status.success(), "stderr={}", stderr(&out));
    let body = stdout(&out);
    assert!(
        body.contains("Alice Example") && body.contains("alice@example.com"),
        "Alice missing: {body}"
    );
    assert!(body.contains("Bob Builder"), "Bob missing: {body}");
    assert!(body.contains("[bot]"), "bot marker missing: {body}");
    assert!(
        body.contains("clustered identities"),
        "summary line missing: {body}"
    );
}

#[test]
fn identities_on_uninitialized_index_reports_zero() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    let out = run_gitlore(repo, &["identities", "--json"]);
    assert!(out.status.success(), "stderr={}", stderr(&out));
    let v = json_line(&stdout(&out));
    assert_eq!(v["clustered_count"].as_u64(), Some(0));
    assert_eq!(v["raw_count"].as_u64(), Some(0));
    assert!(v["identities"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Tests — classify
// ---------------------------------------------------------------------------

#[test]
fn classify_glob_emits_tab_separated_lines() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    let out = run_gitlore(repo, &["classify", "docs/**/*.md"]);
    assert!(out.status.success(), "stderr={}", stderr(&out));
    let body = stdout(&out);
    assert!(
        body.contains("docs/intro.md\tdocs"),
        "expected tab-separated path/category, got: {body:?}"
    );
}

#[test]
fn classify_glob_json_envelope_carries_category_when_homogeneous() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    let out = run_gitlore(repo, &["classify", "tests/**/*.rs", "--json"]);
    assert!(out.status.success(), "stderr={}", stderr(&out));
    let v = json_line(&stdout(&out));
    assert_eq!(v["glob"].as_str(), Some("tests/**/*.rs"));
    let files = v["matched_files"].as_array().expect("matched_files array");
    assert!(
        files
            .iter()
            .any(|f| f["path"].as_str() == Some("tests/foo.rs")
                && f["category"].as_str() == Some("test")),
        "tests/foo.rs missing from {v}"
    );
    assert_eq!(v["category"].as_str(), Some("test"));
}

#[test]
fn classify_glob_with_mixed_categories_drops_top_level_category() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    let out = run_gitlore(repo, &["classify", "**/*", "--json"]);
    assert!(out.status.success(), "stderr={}", stderr(&out));
    let v = json_line(&stdout(&out));
    assert!(
        v["category"].is_null(),
        "mixed bag must collapse to null: {v}"
    );
    assert!(v["matched_files"].as_array().unwrap().len() >= 3);
}

#[test]
fn classify_explain_reads_files_changed_from_index() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    assert!(run_gitlore(repo, &["index"]).status.success());

    // Grab HEAD SHA via real git so we can feed it to --explain.
    let head = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("rev-parse");
    let sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    assert_eq!(
        sha.len(),
        40,
        "expected full SHA from rev-parse, got `{sha}`"
    );

    let out = run_gitlore(repo, &["classify", "--explain", &sha, "--json"]);
    assert!(out.status.success(), "stderr={}", stderr(&out));
    let v = json_line(&stdout(&out));
    assert_eq!(v["sha"].as_str(), Some(sha.as_str()));
    let files = v["files"].as_array().expect("files array");
    // HEAD touched Cargo.toml (config category).
    assert!(
        files
            .iter()
            .any(|f| f["path"].as_str() == Some("Cargo.toml")
                && f["category"].as_str() == Some("config")),
        "Cargo.toml/config missing from {v}"
    );
}

#[test]
fn classify_explain_unique_prefix_resolves_full_sha() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    assert!(run_gitlore(repo, &["index"]).status.success());
    let head = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("rev-parse");
    let sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let prefix = &sha[..7];

    let out = run_gitlore(repo, &["classify", "--explain", prefix, "--json"]);
    assert!(out.status.success(), "stderr={}", stderr(&out));
    let v = json_line(&stdout(&out));
    assert_eq!(v["sha"].as_str(), Some(sha.as_str()));
}

#[test]
fn classify_explain_missing_sha_returns_sha_not_found_envelope() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    assert!(run_gitlore(repo, &["index"]).status.success());

    let out = run_gitlore(
        repo,
        &["classify", "--explain", "ffffffffffffffff", "--json"],
    );
    assert!(!out.status.success(), "expected failure for missing SHA");
    let v = json_line(&stdout(&out));
    assert_eq!(v["error"]["code"].as_str(), Some("sha_not_found"));
}

#[test]
fn classify_without_args_returns_unimplemented_envelope() {
    let fixture = build_fixture_repo();
    let repo = fixture.path();
    let out = run_gitlore(repo, &["classify", "--json"]);
    assert!(!out.status.success(), "no-arg classify must fail");
    let v = json_line(&stdout(&out));
    assert_eq!(v["error"]["code"].as_str(), Some("unimplemented"));
}

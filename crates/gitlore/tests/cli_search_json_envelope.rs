//! JSON envelope validation for `gitlore search --json` (M4 / TDD-001 §4.2).
//!
//! Asserts the SPEC-001 §4.3.1 JSON shape:
//! `{"schema_version":1,"data":{"query":"...","mode":"...","total_available":<u64>,"results":[...]}}`
//! where each result carries `factors.{lexical_bm25, path_relevance, recency}`.

#![allow(clippy::needless_pass_by_value)]

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use assert_cmd::cargo::cargo_bin;
use serde_json::Value;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn build_fixture_repo() -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("gitlore-search-json-")
        .tempdir()
        .expect("tempdir");
    let root = dir.path();
    run_git(root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(root, &["config", "user.email", "search-json@gitlore.dev"]);
    run_git(root, &["config", "user.name", "search-json"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);
    write_file(root, "README.md", "# fixture\n");
    write_file(root, "src/net.rs", "pub fn connect() {}\n");
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "feat: initial"]);
    write_file(
        root,
        "src/net.rs",
        "pub fn connect() {}\npub fn retry() {}\n",
    );
    run_git(root, &["add", "src/net.rs"]);
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
// Test
// ---------------------------------------------------------------------------

#[test]
fn search_json_envelope_satisfies_spec() {
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

    let out = run_gitlore(repo, &["search", "retry", "--json"]);
    assert!(
        out.status.success(),
        "gitlore search --json exited non-zero (code={:?})\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let raw = String::from_utf8_lossy(&out.stdout);

    let v: Value = serde_json::from_str(raw.trim())
        .unwrap_or_else(|e| panic!("search --json output is not valid JSON: {e}\nraw: {raw}"));

    // schema_version == 1.
    assert_eq!(
        v["schema_version"].as_u64(),
        Some(1),
        "schema_version must be 1; envelope: {v}"
    );

    // data object must be present.
    assert!(
        v["data"].is_object(),
        "envelope must have a `data` object; got: {v}"
    );

    let data = &v["data"];

    // data.mode must be a non-null string.
    assert!(
        data["mode"].is_string(),
        "data.mode must be a string; got: {data}"
    );

    // data.results must be an array.
    assert!(
        data["results"].is_array(),
        "data.results must be an array; got: {data}"
    );

    // For every result, assert the three factor fields are present as numbers.
    let results = data["results"].as_array().unwrap();
    for (i, hit) in results.iter().enumerate() {
        let factors = &hit["factors"];
        assert!(
            factors["lexical_bm25"].is_number(),
            "result[{i}].factors.lexical_bm25 is not a number; hit: {hit}"
        );
        assert!(
            factors["path_relevance"].is_number(),
            "result[{i}].factors.path_relevance is not a number; hit: {hit}"
        );
        assert!(
            factors["recency"].is_number(),
            "result[{i}].factors.recency is not a number; hit: {hit}"
        );
    }
}

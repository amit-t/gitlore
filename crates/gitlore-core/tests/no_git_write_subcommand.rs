//! Read-only contract integration test.
//!
//! Builds a wrapper `git` shim at `${TMPDIR}/git-shim/git`, prefixes it onto
//! `PATH`, exercises every [`GitProvider`](gitlore_core::git::GitProvider)
//! method on a tiny fixture repo, then inspects the shim's argv log and
//! asserts every subcommand sits in the read-only allowlist.
//!
//! The shim writes argv (one invocation per line, tab-separated) to a log
//! file then `exec`s the real `git` binary so the tests still observe real
//! behaviour. The fixture repo is bootstrapped with the real `git` binary
//! directly (not through the shim) so setup operations like `init`,
//! `commit`, and `add` aren't logged — only the operations our provider
//! drives.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use gitlore_core::git::cli::GitCliProvider;
use gitlore_core::git::refs::{enumerate_refs, force_push_retention};
use gitlore_core::git::{GitProvider, Sha, ShowOpts, WalkRange};

/// Read-only `git` subcommands a [`GitProvider`] implementation may invoke.
/// Anything else fails the test.
const ALLOWED_SUBCOMMANDS: &[&str] = &[
    "rev-parse",
    "show-ref",
    "for-each-ref",
    "log",
    "show",
    "check-mailmap",
    "cat-file",
    "--version",
    "ls-tree",
    "ls-files",
    "diff-tree",
];

/// Subcommands explicitly blacklisted — even if a future contributor adds
/// them to `ALLOWED_SUBCOMMANDS`, the test still rejects them.
const FORBIDDEN_SUBCOMMANDS: &[&str] = &[
    "init",
    "add",
    "commit",
    "rm",
    "mv",
    "checkout",
    "switch",
    "restore",
    "reset",
    "merge",
    "rebase",
    "cherry-pick",
    "revert",
    "tag",
    "branch",
    "push",
    "pull",
    "fetch",
    "clone",
    "remote",
    "stash",
    "gc",
    "prune",
    "pack-refs",
    "pack-objects",
    "repack",
    "update-ref",
    "write-tree",
    "commit-tree",
    "hash-object",
    "fsck",
    "filter-branch",
    "filter-repo",
    "clean",
    "submodule",
    "worktree",
    "config",
    "notes",
    "replace",
];

/// Locate the real `git` binary so the shim can exec it. We avoid relying on
/// the test's PATH lookup (since PATH gets shimmed) by probing the usual
/// system locations and then falling back to `command -v` *before* the
/// PATH is modified.
fn find_real_git() -> PathBuf {
    for candidate in [
        "/usr/bin/git",
        "/usr/local/bin/git",
        "/opt/homebrew/bin/git",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return p;
        }
    }
    // Last resort: ask the current PATH.
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            let candidate = dir.join("git");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    panic!("could not locate a real git binary on this system");
}

/// Write the shim script and `chmod +x` it.
fn install_shim(shim_dir: &Path, log_path: &Path, real_git: &Path) {
    fs::create_dir_all(shim_dir).expect("create shim dir");
    let shim_path = shim_dir.join("git");
    let script = format!(
        "#!/bin/sh\n\
         # Read-only test shim. Log argv then exec the real git.\n\
         {{\n\
         \tprintf '%s' \"$0\"\n\
         \tfor a in \"$@\"; do printf '\\t%s' \"$a\"; done\n\
         \tprintf '\\n'\n\
         }} >> {log}\n\
         exec {real} \"$@\"\n",
        log = shell_quote(log_path),
        real = shell_quote(real_git),
    );
    fs::write(&shim_path, script).expect("write shim");
    let mut perm = fs::metadata(&shim_path).unwrap().permissions();
    perm.set_mode(0o755);
    fs::set_permissions(&shim_path, perm).expect("chmod shim");
}

fn shell_quote(p: &Path) -> String {
    // Wrap in single quotes and escape embedded single quotes.
    let s = p.to_string_lossy();
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Bootstrap a tiny fixture repo at `repo_dir` using the *real* git binary
/// directly (bypassing the shim so setup isn't logged).
fn bootstrap_fixture_repo(real_git: &Path, repo_dir: &Path) {
    fs::create_dir_all(repo_dir).expect("create repo dir");
    let run = |args: &[&str]| {
        let status = Command::new(real_git)
            .args(args)
            .current_dir(repo_dir)
            // Pin author identity so the test is deterministic on any host
            // and never reads the user's global git config.
            .env("GIT_AUTHOR_NAME", "Tester")
            .env("GIT_AUTHOR_EMAIL", "tester@example.com")
            .env("GIT_COMMITTER_NAME", "Tester")
            .env("GIT_COMMITTER_EMAIL", "tester@example.com")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q", "--initial-branch=main"]);
    fs::write(repo_dir.join("a.txt"), "hello\n").unwrap();
    run(&["add", "a.txt"]);
    run(&[
        "commit",
        "-q",
        "-m",
        "first commit\n\nCo-authored-by: Bob <bob@example.com>",
    ]);
    fs::write(repo_dir.join("a.txt"), "hello\nworld\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-q", "-m", "second commit"]);
    run(&["tag", "-a", "v0.1", "-m", "first tag"]);
}

/// Read and parse the argv log written by the shim. Returns the subcommand
/// (first non-flag arg) for each invocation. Recognises that some
/// invocations may have leading global flags before the subcommand (`-C`,
/// `--git-dir`, `--no-pager`, etc.).
fn read_subcommands(log_path: &Path) -> Vec<String> {
    let text = fs::read_to_string(log_path).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            // First field is the shim's $0 (the shim path). Skip it.
            parts.next()?;
            // Walk past any global flags. Stop at the first arg that
            // doesn't start with '-' OR is one of the well-known flag-form
            // subcommands ("--version", "--exec-path", "--html-path").
            let mut iter = parts.peekable();
            while let Some(arg) = iter.peek() {
                let a = *arg;
                if a == "--version" || a == "--exec-path" || a == "--html-path" {
                    return Some(a.to_string());
                }
                if a.starts_with("-") && !is_flag_subcommand(a) {
                    // Consume the flag (and one value when it's a known
                    // value-taking flag) and continue.
                    let flag = iter.next().unwrap();
                    if flag == "-C" || flag == "--git-dir" || flag == "--work-tree" {
                        iter.next();
                    }
                    continue;
                }
                break;
            }
            iter.next().map(|s| s.to_string())
        })
        .collect()
}

fn is_flag_subcommand(a: &str) -> bool {
    matches!(a, "--version" | "--exec-path" | "--html-path")
}

#[test]
fn provider_methods_only_use_read_only_subcommands() {
    let tmp = env::temp_dir().join(format!(
        "gitlore-no-write-{}-{}",
        std::process::id(),
        nanos_since_epoch()
    ));
    let _cleanup = TempDirGuard(tmp.clone());

    let repo_dir = tmp.join("repo");
    let shim_dir = tmp.join("git-shim");
    let log_path = tmp.join("git-argv.log");

    let real_git = find_real_git();
    bootstrap_fixture_repo(&real_git, &repo_dir);
    install_shim(&shim_dir, &log_path, &real_git);

    // Prepend the shim to PATH. Children of this process inherit the env.
    // This integration test file contains a single #[test], so the binary
    // it compiles to runs that test alone — no intra-binary parallelism to
    // race PATH against.
    let old_path = env::var_os("PATH").unwrap_or_default();
    let mut new_path = std::ffi::OsString::from(&shim_dir);
    new_path.push(":");
    new_path.push(&old_path);
    let _restore_path = RestorePath(old_path);
    env::set_var("PATH", &new_path);

    let provider = GitCliProvider::with_timeout(&repo_dir, Duration::from_secs(15));

    // Exercise every trait method.
    let _common = provider.common_dir().expect("common_dir");
    let head_sha = provider.rev_parse("HEAD").expect("rev_parse HEAD");
    let head_minus = provider.rev_parse("HEAD~1").expect("rev_parse HEAD~1");
    let _heads = provider
        .list_refs(gitlore_core::git::RefScope::Heads)
        .expect("list_refs Heads");
    let _tags = provider
        .list_refs(gitlore_core::git::RefScope::Tags)
        .expect("list_refs Tags");
    let commits = provider
        .walk_commits(WalkRange {
            from: None,
            to: head_sha.clone(),
            max: Some(10),
        })
        .expect("walk_commits");
    assert!(!commits.is_empty(), "walk_commits should return >=1 commit");
    let _diff = provider.show(&head_sha, ShowOpts::default()).expect("show");
    let resolved = provider
        .check_mailmap("Tester", "tester@example.com")
        .expect("check_mailmap");
    assert_eq!(resolved.email, "tester@example.com");
    assert!(provider
        .cat_file_exists(&head_sha)
        .expect("cat_file_exists"));
    let missing = Sha::new("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef").unwrap();
    assert!(!provider.cat_file_exists(&missing).expect("cat_file_exists"));

    // refs helpers.
    let _all = enumerate_refs(&provider).expect("enumerate_refs");
    let orphans = force_push_retention(
        &provider,
        &[head_sha.clone(), head_minus.clone(), missing.clone()],
    )
    .expect("force_push_retention");
    assert_eq!(orphans, vec![missing]);

    // Assert the argv log only contains read-only subcommands.
    let subcommands = read_subcommands(&log_path);
    assert!(
        !subcommands.is_empty(),
        "shim log was empty — was PATH shimmed correctly? log_path={}",
        log_path.display()
    );

    for sub in &subcommands {
        assert!(
            !FORBIDDEN_SUBCOMMANDS.iter().any(|f| f == sub),
            "GitCliProvider invoked forbidden subcommand `git {sub}`; \
             read-only contract violated. Full log:\n{}",
            fs::read_to_string(&log_path).unwrap_or_default()
        );
        assert!(
            ALLOWED_SUBCOMMANDS.iter().any(|a| a == sub),
            "GitCliProvider invoked `git {sub}` which is not in the \
             read-only allowlist. Add it to ALLOWED_SUBCOMMANDS (and verify \
             it's truly read-only) or remove the call. Full log:\n{}",
            fs::read_to_string(&log_path).unwrap_or_default()
        );
    }
}

fn nanos_since_epoch() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct RestorePath(std::ffi::OsString);
impl Drop for RestorePath {
    fn drop(&mut self) {
        env::set_var("PATH", &self.0);
    }
}

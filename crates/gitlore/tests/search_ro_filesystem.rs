//! Read-only filesystem contract for `gitlore search` (AC-RO-1 / M4).
//!
//! After indexing, `gitlore search "retry"` must:
//!
//! 1. Exit with code 0.
//! 2. Leave `git diff --name-only` empty (no tracked files modified).
//! 3. Not invoke any write-side git subcommands during search.

#![allow(clippy::needless_pass_by_value)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

const FORBIDDEN_GIT_SUBCOMMANDS: &[&str] = &[
    "update-ref",
    "add",
    "commit",
    "push",
    "fetch",
    "checkout",
    "gc",
    "reset",
    "merge",
    "rebase",
];

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn build_fixture_repo() -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("gitlore-search-ro-")
        .tempdir()
        .expect("create tempdir");
    let root = dir.path();
    run_git(root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(root, &["config", "user.email", "search-ro@example.invalid"]);
    run_git(root, &["config", "user.name", "Search RO Test"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);
    write_file(root, ".gitignore", "target/\n*.log\n");
    write_file(root, "README.md", "# fixture\n");
    write_file(root, "src/lib.rs", "pub fn one() -> i32 { 1 }\n");
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "feat: initial"]);
    write_file(
        root,
        "src/lib.rs",
        "pub fn one() -> i32 { 1 }\npub fn retry() {}\n",
    );
    run_git(root, &["add", "src/lib.rs"]);
    run_git(root, &["commit", "--quiet", "-m", "fix: retry on timeout"]);
    dir
}

fn write_file(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir parent");
    }
    fs::write(&path, contents).expect("write fixture file");
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    assert!(
        status.success(),
        "git {:?} failed in {}",
        args,
        cwd.display()
    );
}

// ---------------------------------------------------------------------------
// gitlore runner
// ---------------------------------------------------------------------------

fn run_gitlore(repo: &Path, args: &[&str], shim: Option<&GitShim>) -> std::process::Output {
    let bin = cargo_bin("gitlore");
    let mut cmd = Command::new(&bin);
    cmd.current_dir(repo)
        .args(args)
        .env("HOME", repo.parent().unwrap_or(repo))
        .env(
            "XDG_CONFIG_HOME",
            repo.parent().unwrap_or(repo).join("xdg-config"),
        )
        .env(
            "XDG_DATA_HOME",
            repo.parent().unwrap_or(repo).join("xdg-data"),
        )
        .env(
            "XDG_CACHE_HOME",
            repo.parent().unwrap_or(repo).join("xdg-cache"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(s) = shim {
        cmd.env("PATH", s.path_prefix());
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("spawn gitlore {args:?}: {e}"))
}

// ---------------------------------------------------------------------------
// PATH shim (mirrors ro_filesystem.rs GitShim)
// ---------------------------------------------------------------------------

struct GitShim {
    bin_dir: PathBuf,
    log_path: PathBuf,
    _scratch: TempDir,
}

impl GitShim {
    fn install() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("gitlore-search-ro-shim-")
            .tempdir()
            .expect("create shim scratch");
        let bin_dir = scratch.path().join("bin");
        let log_path = scratch.path().join("git-argv.log");
        fs::create_dir_all(&bin_dir).expect("create shim bin dir");
        fs::write(&log_path, b"").expect("touch shim log");

        let real_git = find_real_git();
        let script = format!(
            "#!/bin/sh\n\
             {{\n\
             \tprintf '%s' \"$0\"\n\
             \tfor a in \"$@\"; do printf '\\t%s' \"$a\"; done\n\
             \tprintf '\\n'\n\
             }} >> {log}\n\
             exec {real} \"$@\"\n",
            log = sh_quote(&log_path),
            real = sh_quote(&real_git),
        );
        let shim_path = bin_dir.join("git");
        fs::write(&shim_path, script).expect("write shim script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = fs::metadata(&shim_path).expect("stat shim").permissions();
            perm.set_mode(0o755);
            fs::set_permissions(&shim_path, perm).expect("chmod shim");
        }
        Self {
            bin_dir,
            log_path,
            _scratch: scratch,
        }
    }

    fn truncate_log(&self) {
        fs::write(&self.log_path, b"").expect("truncate shim log");
    }

    fn forbidden_subcommands_seen(&self) -> Vec<String> {
        read_subcommands(&self.log_path)
            .into_iter()
            .filter(|s| FORBIDDEN_GIT_SUBCOMMANDS.iter().any(|f| *f == s))
            .collect()
    }

    fn path_prefix(&self) -> OsString {
        let mut new_path = OsString::from(&self.bin_dir);
        if let Some(existing) = std::env::var_os("PATH") {
            new_path.push(":");
            new_path.push(&existing);
        }
        new_path
    }
}

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
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("git");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    panic!("could not locate a real git binary on this system");
}

fn sh_quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

fn read_subcommands(log_path: &Path) -> Vec<String> {
    let text = fs::read_to_string(log_path).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            // First field is $0 (the shim path). Skip it.
            parts.next()?;
            let mut iter = parts.peekable();
            while let Some(arg) = iter.peek() {
                let a = *arg;
                if a == "--version" || a == "--exec-path" || a == "--html-path" {
                    return Some(a.to_string());
                }
                if a.starts_with('-') {
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

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[test]
fn search_does_not_modify_git_tracked_files() {
    let fixture = build_fixture_repo();
    let repo = fixture.path().to_path_buf();

    let shim = GitShim::install();

    // Pre-index (shim active so git calls are logged, but we ignore them here).
    let idx_out = run_gitlore(&repo, &["index"], Some(&shim));
    assert!(
        idx_out.status.success(),
        "pre-index failed (exit={:?})\nstdout={}\nstderr={}",
        idx_out.status.code(),
        String::from_utf8_lossy(&idx_out.stdout),
        String::from_utf8_lossy(&idx_out.stderr),
    );

    // Clear the log so only search's git calls are captured below.
    shim.truncate_log();

    // Run `gitlore search "retry"`.
    let out = run_gitlore(&repo, &["search", "retry"], Some(&shim));
    assert!(
        out.status.success(),
        "gitlore search exited non-zero (code={:?})\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // AC-RO-1: no write-side git subcommands during search.
    let forbidden = shim.forbidden_subcommands_seen();
    assert!(
        forbidden.is_empty(),
        "`gitlore search` invoked write-side git subcommand(s) {forbidden:?}; \
         RO contract violated. Shim log:\n{}",
        fs::read_to_string(&shim.log_path).unwrap_or_default(),
    );

    // AC-RO-2: `git diff --name-only` must be empty.
    let diff_out = Command::new("git")
        .current_dir(&repo)
        .args(["diff", "--name-only"])
        .output()
        .expect("spawn git diff");
    let diff_stdout = String::from_utf8_lossy(&diff_out.stdout);
    assert!(
        diff_stdout.trim().is_empty(),
        "git diff reports modified tracked files after `gitlore search`:\n{diff_stdout}"
    );

    // Belt-and-suspenders: no unexpected untracked files in worktree.
    let status_out = Command::new("git")
        .current_dir(&repo)
        .args(["status", "--porcelain"])
        .output()
        .expect("spawn git status");
    let porcelain = String::from_utf8_lossy(&status_out.stdout);
    let unexpected: Vec<&str> = porcelain
        .lines()
        .filter(|l| !l.contains(".gitlore") && !l.contains("xdg-"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "`git status --porcelain` reports unexpected worktree changes after \
         `gitlore search`:\n{}",
        unexpected.join("\n")
    );
}

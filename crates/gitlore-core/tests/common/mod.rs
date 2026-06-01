//! Shared helpers for the M3-6 indexer integration tests.
//!
//! Each test drives [`gitlore_core::index::indexer::Indexer`] against a
//! synthetic on-disk repository built here by shelling out to `git`. The
//! repos are tiny (≤20 commits) so the integration tests stay fast and
//! deterministic; the larger 1500-commit resume fixture is generated
//! inline by `index_resume_after_sigkill` and gated behind `#[ignore]`
//! until the private QA fixtures land at M3-7.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// Owned tempdir + repo root for a synthetic Git fixture.
pub struct Fixture {
    pub dir: TempDir,
    pub repo: PathBuf,
}

impl Fixture {
    /// Initialise a fresh repo with deterministic identity + main branch.
    pub fn init() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().to_path_buf();
        run_git(&repo, &["init", "--quiet", "--initial-branch=main"]);
        run_git(&repo, &["config", "user.email", "test@example.com"]);
        run_git(&repo, &["config", "user.name", "Test User"]);
        run_git(&repo, &["config", "commit.gpgsign", "false"]);
        run_git(&repo, &["config", "tag.gpgsign", "false"]);
        Self { dir, repo }
    }

    /// Write `path` with `content`, stage, and create a commit with
    /// `message`. Returns the freshly minted commit's full SHA.
    pub fn commit(&self, path: &str, content: &str, message: &str) -> String {
        let full = self.repo.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p");
        }
        std::fs::write(&full, content).expect("write");
        run_git(&self.repo, &["add", path]);
        run_git(&self.repo, &["commit", "--quiet", "-m", message]);
        head_sha(&self.repo)
    }

    /// Create a branch off the current HEAD, switch to it.
    pub fn checkout_new_branch(&self, name: &str) {
        run_git(&self.repo, &["checkout", "--quiet", "-b", name]);
    }

    /// Switch to an existing branch.
    pub fn checkout(&self, name: &str) {
        run_git(&self.repo, &["checkout", "--quiet", name]);
    }

    /// Create a tag pointing at HEAD. `annotated=true` produces an
    /// annotated tag with a message.
    pub fn tag(&self, name: &str, annotated: bool) {
        if annotated {
            run_git(
                &self.repo,
                &["tag", "-a", name, "-m", &format!("release {name}")],
            );
        } else {
            run_git(&self.repo, &["tag", name]);
        }
    }

    /// Hard-reset the current branch to `target` (sha or ref).
    pub fn reset_hard(&self, target: &str) {
        run_git(&self.repo, &["reset", "--quiet", "--hard", target]);
    }

    /// Expire reflog + GC unreachable objects so force-pushed commits
    /// are actually unreachable via `cat-file -e`.
    pub fn gc_now(&self) {
        run_git(
            &self.repo,
            &[
                "reflog",
                "expire",
                "--expire=now",
                "--expire-unreachable=now",
                "--all",
            ],
        );
        run_git(&self.repo, &["gc", "--prune=now", "--quiet"]);
    }

    /// Resolve the commit currently at `HEAD`.
    pub fn head(&self) -> String {
        head_sha(&self.repo)
    }
}

/// Run `git <args>` inside `dir` and panic with the captured stderr if
/// the subprocess exits non-zero.
pub fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: spawn failed: {e}"));
    if !out.status.success() {
        panic!(
            "git {:?} exited {:?}:\nstdout={}\nstderr={}",
            args,
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
}

/// Capture stdout of `git <args>` inside `dir` as a trimmed string.
pub fn capture_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: spawn failed: {e}"));
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub fn head_sha(dir: &Path) -> String {
    capture_git(dir, &["rev-parse", "HEAD"])
}

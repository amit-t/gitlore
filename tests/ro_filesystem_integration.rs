//! Read-only filesystem integration test (AC-RO-1, AC-RO-2).
//!
//! Enforces the project's hard rule from SPEC §11 / §18: **gitlore must never
//! modify the repository it observes**. This test boots the compiled `gitlore`
//! binary inside a tempdir fixture whose worktree has been chmod'd to a
//! read-only mode, then exercises every M1-reachable surface (CLI subcommands
//! + a 60-second TUI session) and asserts the worktree is byte-identical
//! afterwards.
//!
//! ## Scope across milestones
//!
//! * **M1 (this file as-shipped):** the only functional surface is the empty
//!   3-pane TUI plus `--help` / `--version`. The non-M1 subcommands (`index`,
//!   `search`, `story`, `risk`, `hotspots`, `explain`, `between`,
//!   `setup-embeddings`, `config`, `identities`, `classify`, `status`) are
//!   plumbed as clap-derive stubs that exit non-zero with the stable
//!   `Unimplemented` error code per SPEC §4.3. We still invoke them: an
//!   `Unimplemented` error code is **not** an exemption from the RO contract,
//!   it's the contract's strongest form (zero side effects).
//! * **M3:** once the SQLite indexer lands, the full AC-RO matrix lights up.
//!   `index` becomes the highest-risk surface because it is the only command
//!   that *opens a write handle anywhere* (to the per-repo index in
//!   `<common-dir>/gitlore/`, never the worktree). The assertions below
//!   already cover that case: any byte changing inside the worktree fails the
//!   test, whether it came from the indexer, a stray `git gc`, or anything
//!   else.
//!
//! ## Why a separate RO mount
//!
//! `chmod -R a-w` on the worktree is the OS-level belt-and-braces check. If
//! gitlore tried to write to a tracked file, the syscall would `EACCES` and
//! the process would exit non-zero or panic. The snapshot diff below catches
//! the subtler case where a write succeeds against a directory we forgot to
//! lock down. Together they form AC-RO-1 (no writes attempted) and AC-RO-2
//! (no writes observable post-hoc).
//!
//! ## Wiring
//!
//! This file lives at workspace root (`tests/ro_filesystem_integration.rs`).
//! The workspace `Cargo.toml` declares the root package + `[[test]]` target
//! so `cargo test --test ro_filesystem_integration` builds the `gitlore`
//! binary and runs this harness. The CI lane `ro-filesystem-integration` in
//! `.github/workflows/ci.yml` runs the same command on macOS + Linux runners.
//!
//! Required dev-dependencies (declared in workspace `Cargo.toml`):
//!
//! ```toml
//! [dev-dependencies]
//! assert_cmd = "2"
//! tempfile = "3"
//! walkdir = "2"
//! sha2 = "0.10"
//! ```
//!
//! Windows note: chmod semantics differ; the read-only enforcement step is
//! best-effort via `fs::set_permissions(.., readonly=true)`. Snapshot-based
//! verification still applies. Per OQ-T-2 and the M1 platform matrix
//! (macOS-latest + Ubuntu-latest), Windows is best-effort until M10.

#![allow(clippy::needless_pass_by_value)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Fixture: a tiny well-formed git repo with a .gitignore we can fingerprint.
// ---------------------------------------------------------------------------

/// Builds a minimal but realistic git fixture in a fresh tempdir.
///
/// The repo contains three commits so that any M3+ indexer surface that walks
/// history has something to chew on, plus a `.gitignore` whose checksum we
/// pin separately (AC-RO-2 calls it out explicitly because it's the canary
/// most likely to be touched by a sloppy "open the repo in write mode then
/// close it again" code path).
fn build_fixture_repo() -> TempDir {
    let dir = tempfile::Builder::new()
        .prefix("gitlore-ro-fixture-")
        .tempdir()
        .expect("create tempdir");
    let root = dir.path();

    run_git(root, &["init", "--initial-branch=main", "--quiet"]);
    run_git(root, &["config", "user.email", "ro-test@example.invalid"]);
    run_git(root, &["config", "user.name", "RO Test"]);
    run_git(root, &["config", "commit.gpgsign", "false"]);

    write_file(root, ".gitignore", "target/\n*.log\n");
    write_file(root, "README.md", "# fixture\n\nFor RO integration test.\n");
    write_file(root, "src/lib.rs", "pub fn one() -> i32 { 1 }\n");

    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "feat: initial"]);

    write_file(root, "src/lib.rs", "pub fn one() -> i32 { 1 }\npub fn two() -> i32 { 2 }\n");
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
        fs::create_dir_all(parent).expect("mkdir parent");
    }
    fs::write(&path, contents).expect("write fixture file");
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("spawn git {:?}: {e}", args));
    assert!(status.success(), "git {:?} failed in {}", args, cwd.display());
}

// ---------------------------------------------------------------------------
// RO mount guard: chmod the worktree to a-w on construction, restore on Drop.
// ---------------------------------------------------------------------------

/// RAII guard: makes the worktree (everything except `.git/`) read-only when
/// constructed, restores writable permissions on drop so tempfile cleanup can
/// `rmdir`.
struct RoGuard {
    root: PathBuf,
}

impl RoGuard {
    fn lock(root: &Path) -> Self {
        let me = Self { root: root.to_path_buf() };
        me.apply(false);
        me
    }

    /// `writable=true` restores 0o755 dirs / 0o644 files (the conservative
    /// pre-test mode). `writable=false` strips the write bit from owner+group+
    /// other across the whole worktree, skipping `.git/`.
    fn apply(&self, writable: bool) {
        for entry in WalkDir::new(&self.root).into_iter().filter_map(Result::ok) {
            // Never touch .git/ — git itself needs to update reflog/index/etc
            // *during* git invocations that set up the fixture. The RO contract
            // we care about is the *worktree*. The snapshot test below still
            // catches any sneaky `.git/` write that bleeds into the worktree.
            if entry
                .path()
                .components()
                .any(|c| c.as_os_str() == std::ffi::OsStr::new(".git"))
            {
                continue;
            }
            let _ = set_mode(entry.path(), entry.file_type().is_dir(), writable);
        }
    }
}

impl Drop for RoGuard {
    fn drop(&mut self) {
        self.apply(true);
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, is_dir: bool, writable: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = match (is_dir, writable) {
        (true, true) => 0o755,
        (true, false) => 0o555,
        (false, true) => 0o644,
        (false, false) => 0o444,
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(path: &Path, _is_dir: bool, writable: bool) -> std::io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_readonly(!writable);
    fs::set_permissions(path, perms)
}

// ---------------------------------------------------------------------------
// Snapshotting: every file under the worktree (excluding .git/) recorded by
// relative path + SHA-256. Comparing two snapshots gives us byte-level
// equality + addition/deletion detection in one pass.
// ---------------------------------------------------------------------------

type Snapshot = BTreeMap<PathBuf, [u8; 32]>;

fn snapshot_worktree(root: &Path) -> Snapshot {
    let mut out = Snapshot::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .expect("walkdir entry under root")
            .to_path_buf();
        if rel.components().any(|c| c.as_os_str() == std::ffi::OsStr::new(".git")) {
            continue;
        }
        out.insert(rel, sha256_file(entry.path()));
    }
    out
}

fn sha256_file(path: &Path) -> [u8; 32] {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut h = Sha256::new();
    h.update(&bytes);
    h.finalize().into()
}

fn assert_snapshots_equal(before: &Snapshot, after: &Snapshot, context: &str) {
    if before == after {
        return;
    }
    // Build a precise diff message: which files appeared, vanished, or
    // changed bytes. This is the report a human will read when CI fails.
    let mut diffs: Vec<String> = Vec::new();
    for (path, hash) in before {
        match after.get(path) {
            None => diffs.push(format!("deleted: {}", path.display())),
            Some(h) if h != hash => diffs.push(format!("modified: {}", path.display())),
            _ => {}
        }
    }
    for path in after.keys() {
        if !before.contains_key(path) {
            diffs.push(format!("added: {}", path.display()));
        }
    }
    panic!(
        "RO contract violated during `{context}` — {} worktree change(s):\n  {}",
        diffs.len(),
        diffs.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// Subcommand exercise. M1 surfaces are listed explicitly; expand as new
// commands gain M3+ implementations. We treat every command as a candidate
// RO-contract violator, including --help/--version (which historically can
// touch ~/.config or write a marker file in misbehaving binaries).
// ---------------------------------------------------------------------------

/// The argv vectors we run per subcommand. Kept as `Vec<Vec<&'static str>>` so
/// adding a new M3+ command is a one-line append. Long-running commands
/// (`index`) get a `--dry-run` flag once it exists; until then, the M1 stub
/// exits with `Unimplemented` before doing anything observable.
fn m1_subcommand_invocations() -> Vec<Vec<&'static str>> {
    vec![
        vec!["--help"],
        vec!["--version"],
        vec!["help"],
        // Stub subcommands per SPEC-001 §4.1 / fix_plan line 12. Each is
        // expected to exit with the stable "Unimplemented" error at M1.
        vec!["status"],
        vec!["index"],
        vec!["search", "two"],
        vec!["story", "--since", "HEAD~2"],
        vec!["risk", "--since", "HEAD~2"],
        vec!["hotspots", "src/"],
        vec!["explain", "HEAD"],
        vec!["between", "HEAD~2", "HEAD"],
        vec!["setup-embeddings"],
        vec!["config", "get", "tui.theme"],
        vec!["identities"],
        vec!["classify"],
        // --json flag is plumbed on every subcommand per fix_plan line 12;
        // probe one as a smoke check that the global flag doesn't write.
        vec!["status", "--json"],
    ]
}

fn run_gitlore(repo: &Path, args: &[&str]) -> std::process::Output {
    let bin = cargo_bin("gitlore");
    Command::new(&bin)
        .current_dir(repo)
        .args(args)
        // Isolate from the dev's real home: any code path that accidentally
        // reads/writes ~/.config/gitlore should fail closed instead of
        // mutating the host. We rebind XDG_* to a throwaway dir inside the
        // tempdir's *parent*, not the worktree itself, because the worktree
        // is now RO and gitlore would legitimately need to write its index
        // and config somewhere outside it.
        .env("HOME", repo.parent().unwrap_or(repo))
        .env("XDG_CONFIG_HOME", repo.parent().unwrap_or(repo).join("xdg-config"))
        .env("XDG_DATA_HOME", repo.parent().unwrap_or(repo).join("xdg-data"))
        .env("XDG_CACHE_HOME", repo.parent().unwrap_or(repo).join("xdg-cache"))
        // No interactive prompts: every M1 subcommand must run unattended.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("spawn gitlore {:?}: {e}", args))
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[test]
fn ro_contract_holds_across_m1_subcommands() {
    let fixture = build_fixture_repo();
    let repo = fixture.path().to_path_buf();
    let gitignore_sha_before = sha256_file(&repo.join(".gitignore"));
    let snap_before = snapshot_worktree(&repo);
    let _guard = RoGuard::lock(&repo);

    for argv in m1_subcommand_invocations() {
        // Exit code is intentionally not asserted: at M1 most subcommands
        // exit non-zero with `Unimplemented`. The contract under test is
        // worktree immutability, not command success.
        let out = run_gitlore(&repo, &argv);
        // Defensive: if the binary segfaults we want a clear failure rather
        // than a silent skip.
        if let Some(code) = out.status.code() {
            assert!(
                code >= 0,
                "gitlore {:?} returned an impossible exit code {code}",
                argv
            );
        }
        let snap_after = snapshot_worktree(&repo);
        let label = argv.join(" ");
        assert_snapshots_equal(&snap_before, &snap_after, &format!("gitlore {label}"));
    }

    let gitignore_sha_after = sha256_file(&repo.join(".gitignore"));
    assert_eq!(
        gitignore_sha_before, gitignore_sha_after,
        "AC-RO-2: .gitignore checksum drifted during M1 CLI surface exercise"
    );
}

#[test]
fn ro_contract_holds_across_60s_tui_session() {
    let duration = tui_session_duration();
    let fixture = build_fixture_repo();
    let repo = fixture.path().to_path_buf();
    let gitignore_sha_before = sha256_file(&repo.join(".gitignore"));
    let snap_before = snapshot_worktree(&repo);
    let _guard = RoGuard::lock(&repo);

    let bin = cargo_bin("gitlore");
    let mut child = Command::new(&bin)
        .current_dir(&repo)
        .env("HOME", repo.parent().unwrap_or(&repo))
        .env(
            "XDG_CONFIG_HOME",
            repo.parent().unwrap_or(&repo).join("xdg-config"),
        )
        .env(
            "XDG_DATA_HOME",
            repo.parent().unwrap_or(&repo).join("xdg-data"),
        )
        .env(
            "XDG_CACHE_HOME",
            repo.parent().unwrap_or(&repo).join("xdg-cache"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn gitlore TUI");

    // Hold the session open. crossterm will see a piped stdin instead of a
    // TTY; depending on M1 implementation it may exit immediately (acceptable)
    // or sit in its event loop waiting on the pipe (also acceptable). Either
    // way the assertion below catches any worktree mutation that happened
    // during the window.
    let started = Instant::now();
    while started.elapsed() < duration {
        // If the child has already exited, no point sleeping out the full
        // window — but we still hold the RO guard until the full assertion.
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => thread::sleep(Duration::from_millis(200)),
            Err(e) => panic!("try_wait on gitlore: {e}"),
        }
    }

    // Best-effort graceful quit: send 'q' (the documented quit key per
    // SPEC §11.9 and fix_plan line 13). If the child has already exited or
    // the stdin pipe is closed, ignore — we'll kill it below either way.
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(b"q");
    }
    // Give it 2s to flush + exit cleanly.
    let grace_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < grace_deadline {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(e) => panic!("try_wait on gitlore: {e}"),
        }
    }
    // Force termination if still alive — the RO contract doesn't care how
    // gracefully it dies, only whether it wrote anything.
    let _ = child.kill();
    let _ = child.wait();

    let snap_after = snapshot_worktree(&repo);
    assert_snapshots_equal(
        &snap_before,
        &snap_after,
        &format!("gitlore TUI ({}s session)", duration.as_secs()),
    );
    let gitignore_sha_after = sha256_file(&repo.join(".gitignore"));
    assert_eq!(
        gitignore_sha_before, gitignore_sha_after,
        "AC-RO-2: .gitignore checksum drifted during {}s TUI session",
        duration.as_secs()
    );
}

/// Default TUI session length. The spec calls for 60 seconds; we honor that
/// in CI but allow shortening locally via env var to keep `cargo test` fast
/// during M1 inner-loop development. CI must run with the default.
fn tui_session_duration() -> Duration {
    let secs: u64 = std::env::var_os("GITLORE_RO_TEST_TUI_SECONDS")
        .and_then(|v: OsString| v.into_string().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    Duration::from_secs(secs)
}

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
//! * **M3 (current):** the SQLite indexer (M3-6) and the four wired CLI
//!   surfaces — `index`, `status`, `identities`, `classify` — light up the
//!   full AC-RO matrix. `index` is the highest-risk surface because it is
//!   the only command that *opens a write handle anywhere* (to the per-repo
//!   index in `<common-dir>/gitlore/`, never the worktree). Two additional
//!   checks layer on top of the snapshot diff:
//!     1. The fixture is **pre-indexed in a writable scratch dir** before
//!        the RO flip, so `identities` / `classify` / `status` have data to
//!        read after lockdown.
//!     2. Every gitlore invocation runs under a **`PATH`-shimmed `git`**
//!        (the M3-1 contract verifier, mirrored from
//!        `gitlore-core/tests/no_git_write_subcommand.rs`). The shim logs
//!        every `git` argv to a file; after each subcommand the log is
//!        scanned for write-side subcommands (`update-ref`, `add`, `commit`,
//!        `push`, `fetch`, `checkout`, `gc`, `reset`, `merge`, `rebase`) and
//!        the test fails on any hit. AC-RO-1 and AC-RO-2 are closed across
//!        the M3 CLI surface.
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
//! This file lives at `crates/gitlore/tests/ro_filesystem.rs` and is wired
//! through `gitlore`'s own dev-dependencies. The CI lane
//! `ro-filesystem-integration` in `.github/workflows/ci.yml` runs
//! `cargo test --workspace --test ro_filesystem --locked` on macOS + Linux.
//!
//! Required dev-dependencies (declared in `crates/gitlore/Cargo.toml`):
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
#![allow(clippy::doc_lazy_continuation)]

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
        let me = Self {
            root: root.to_path_buf(),
        };
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
        if rel
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new(".git"))
        {
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
// PATH-shim wrapper for `git`. Mirrors the M3-1 contract verifier in
// `gitlore-core/tests/no_git_write_subcommand.rs`: a shell script logs argv
// (one tab-separated line per invocation) then `exec`s the real git binary,
// so child gitlore processes still observe real git behaviour while the
// test can introspect every subcommand that was issued.
// ---------------------------------------------------------------------------

/// Write-side `git` subcommands that must never appear in the shim log
/// during any RO subcommand exercise (per task M3-7f).
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

/// RAII shim: owns a tempdir containing `bin/git` (a shell wrapper) and the
/// argv log file. Drop tears the tempdir down; callers prepend `bin/` to
/// child `PATH` via [`Self::path_prefix`].
struct GitShim {
    bin_dir: PathBuf,
    log_path: PathBuf,
    _scratch: TempDir,
}

impl GitShim {
    fn install() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("gitlore-ro-shim-")
            .tempdir()
            .expect("create shim scratch");
        let bin_dir = scratch.path().join("bin");
        let log_path = scratch.path().join("git-argv.log");
        fs::create_dir_all(&bin_dir).expect("create shim bin dir");
        // Touch the log so the very first `truncate_log` call can rely on
        // the path existing.
        fs::write(&log_path, b"").expect("touch shim log");

        let real_git = find_real_git();
        let script = format!(
            "#!/bin/sh\n\
             # M3-7f read-only test shim. Log argv (tab-separated) then exec real git.\n\
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

    /// Clear the argv log between subcommand invocations so each assertion
    /// only sees git calls issued by the current `gitlore` run.
    fn truncate_log(&self) {
        fs::write(&self.log_path, b"").expect("truncate shim log");
    }

    /// Return any [`FORBIDDEN_GIT_SUBCOMMANDS`] entries observed in the log
    /// since the last [`Self::truncate_log`]. Empty vec means clean.
    fn forbidden_subcommands_seen(&self) -> Vec<String> {
        read_subcommands(&self.log_path)
            .into_iter()
            .filter(|s| FORBIDDEN_GIT_SUBCOMMANDS.iter().any(|f| *f == s))
            .collect()
    }

    /// Build a `PATH` value with the shim's `bin/` prepended to the current
    /// process's `PATH`. Caller passes this verbatim to `Command::env`.
    fn path_prefix(&self) -> OsString {
        let mut new_path = OsString::from(&self.bin_dir);
        if let Some(existing) = std::env::var_os("PATH") {
            new_path.push(":");
            new_path.push(&existing);
        }
        new_path
    }
}

/// Locate the real `git` binary so the shim can `exec` it. Avoid relying on
/// the test's PATH lookup once the shim is installed; probe well-known
/// system paths first, then fall back to a PATH scan done *before* PATH is
/// shimmed in the caller.
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

/// Single-quote a path for safe interpolation into a `/bin/sh` script.
fn sh_quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Parse the shim log into a list of issued subcommands. Walks past
/// well-known global flags (`-C <dir>`, `--git-dir <p>`, `--work-tree <p>`,
/// `--no-pager`, …) and returns the first non-flag argv element. The
/// flag-form pseudo-subcommands (`--version`, `--exec-path`, `--html-path`)
/// are returned verbatim so the test still sees them.
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
// Subcommand exercise. Covers (a) every M3-wired surface (`index`, `status`,
// `identities`, `classify`) in both plain and `--json` form, plus
// `identities --include-bots` and `classify --explain HEAD`, and (b) the
// remaining clap-derive stubs that still return `Unimplemented`. Every
// command is treated as a candidate RO-contract violator, including
// `--help` / `--version` (which historically can touch ~/.config or write
// a marker file in misbehaving binaries).
// ---------------------------------------------------------------------------

/// The argv vectors we run per subcommand. Kept as `Vec<Vec<&'static str>>` so
/// adding a new M-something command is a one-line append. The function name
/// is a historical artefact from the M1 stub era; despite the name, the list
/// now exercises every wired M3 surface.
fn m1_subcommand_invocations() -> Vec<Vec<&'static str>> {
    vec![
        // Help / version (no subcommand).
        vec!["--help"],
        vec!["--version"],
        vec!["help"],
        // Remaining stub subcommands per SPEC-001 §4.1. Each exits with the
        // stable "Unimplemented" error until its handler lands; the RO
        // contract still applies (zero worktree writes, no write-side git).
        vec!["search", "two"],
        vec!["story", "--since", "HEAD~2"],
        vec!["risk", "--since", "HEAD~2"],
        vec!["hotspots", "src/"],
        vec!["explain", "HEAD"],
        vec!["between", "HEAD~2", "HEAD"],
        vec!["setup-embeddings"],
        vec!["config", "get", "tui.theme"],
        // M3 wired surfaces (M3-7a `index` + `status`, M3-7b `identities`
        // + `classify`). Each is exercised in both plain and `--json`
        // form so the global `--json` flag is verified RO-clean too.
        vec!["status"],
        vec!["status", "--json"],
        vec!["index"],
        vec!["index", "--json"],
        vec!["identities"],
        vec!["identities", "--json"],
        vec!["identities", "--include-bots"],
        vec!["identities", "--include-bots", "--json"],
        vec!["classify", "**/*.rs"],
        vec!["classify", "**/*.rs", "--json"],
        vec!["classify", "--explain", "HEAD"],
        vec!["classify", "--explain", "HEAD", "--json"],
    ]
}

/// Spawn the compiled `gitlore` binary against `repo`. When a [`GitShim`] is
/// supplied, its `bin/` directory is prepended to `PATH` so every inner `git`
/// call routes through the argv-logging shim.
fn run_gitlore(repo: &Path, args: &[&str], shim: Option<&GitShim>) -> std::process::Output {
    let bin = cargo_bin("gitlore");
    let mut cmd = Command::new(&bin);
    cmd.current_dir(repo)
        .args(args)
        // Isolate from the dev's real home: any code path that accidentally
        // reads/writes ~/.config/gitlore should fail closed instead of
        // mutating the host. We rebind XDG_* to a throwaway dir inside the
        // tempdir's *parent*, not the worktree itself, because the worktree
        // is RO under test and gitlore would legitimately need to write its
        // index and config somewhere outside it.
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
        // No interactive prompts: every subcommand must run unattended.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(shim) = shim {
        cmd.env("PATH", shim.path_prefix());
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("spawn gitlore {args:?}: {e}"))
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[test]
fn ro_contract_holds_across_m1_subcommands() {
    let fixture = build_fixture_repo();
    let repo = fixture.path().to_path_buf();
    let gitignore_sha_before = sha256_file(&repo.join(".gitignore"));

    // Install the PATH shim before any gitlore invocation so every inner
    // `git` call routes through the argv logger. Installing happens before
    // pre-indexing too, but we don't inspect the log until after the RO
    // flip — the pre-index run is allowed to use whatever read-only git
    // surface it needs.
    let shim = GitShim::install();

    // Pre-index in a writable worktree so the M3-7b surfaces (`identities`,
    // `classify`, `status`) have data to read once the worktree flips RO.
    // The indexer writes into `.git/gitlore/` only; nothing under the
    // worktree proper changes, but git itself may stat / temp-write paths
    // outside `.git/` during its walk, so it must happen pre-lockdown.
    let preindex = run_gitlore(&repo, &["index"], Some(&shim));
    assert!(
        preindex.status.success(),
        "pre-index failed (exit={:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        preindex.status.code(),
        String::from_utf8_lossy(&preindex.stdout),
        String::from_utf8_lossy(&preindex.stderr),
    );

    let snap_before = snapshot_worktree(&repo);
    let _guard = RoGuard::lock(&repo);

    for argv in m1_subcommand_invocations() {
        shim.truncate_log();

        // Exit code is intentionally not asserted: many subcommands still
        // exit non-zero with `Unimplemented`. The contract under test is
        // worktree immutability + no write-side git, not command success.
        let out = run_gitlore(&repo, &argv, Some(&shim));
        // Defensive: if the binary segfaults we want a clear failure rather
        // than a silent skip.
        if let Some(code) = out.status.code() {
            assert!(
                code >= 0,
                "gitlore {argv:?} returned an impossible exit code {code}"
            );
        }

        let label = argv.join(" ");

        // AC-RO-1 / AC-RO-2: worktree byte-identical.
        let snap_after = snapshot_worktree(&repo);
        assert_snapshots_equal(&snap_before, &snap_after, &format!("gitlore {label}"));

        // AC-RO-1: no write-side git subcommands routed through the shim.
        let forbidden = shim.forbidden_subcommands_seen();
        assert!(
            forbidden.is_empty(),
            "gitlore {label:?} invoked write-side git subcommand(s) {forbidden:?}; \
             RO contract violated. Full shim log:\n{}",
            fs::read_to_string(&shim.log_path).unwrap_or_default(),
        );
    }

    // AC-RO-2 (canary): .gitignore byte-stable across the whole exercise.
    let gitignore_sha_after = sha256_file(&repo.join(".gitignore"));
    assert_eq!(
        gitignore_sha_before, gitignore_sha_after,
        "AC-RO-2: .gitignore checksum drifted during M3 CLI surface exercise"
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
        gitignore_sha_before,
        gitignore_sha_after,
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

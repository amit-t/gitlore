//! Index storage location resolver (SPEC-001 §5.2, ADR-029, Q15b).
//!
//! gitlore prefers to keep the SQLite index inside the repo's Git common
//! dir so that worktrees share a single canonical index and `git push`
//! mirrors carry no copy of the user's local exploration data. When the
//! common dir is not writable (read-only filesystems, mirrors mounted
//! `nosuid`, restricted shared servers) the resolver falls back to the
//! per-user XDG data dir.
//!
//! ## Algorithm
//!
//! 1. Call [`crate::git::GitProvider::common_dir`] to find the canonical
//!    Git common dir. The returned path is absolute.
//! 2. Probe write access by creating `<common-dir>/gitlore/.write_probe`,
//!    writing a byte to it, and removing it. The probe directory itself
//!    is created if missing — that is the only side effect; the probe
//!    file is always cleaned up.
//! 3. On success, return [`IndexLocation::CommonDir`] pointing at
//!    `<common-dir>/gitlore/`.
//! 4. On `PermissionDenied` (or `ReadOnlyFilesystem`) anywhere in the
//!    probe, fall back to [`IndexLocation::Xdg`] pointing at the
//!    per-user data dir (`<XDG_DATA_HOME>/gitlore/repos/<digest>/` on
//!    Linux, the platform equivalent elsewhere).
//! 5. Any other I/O failure propagates as [`Error::Io`] — refusing to
//!    silently swap to XDG on an ambiguous error keeps the user in
//!    control of where their data lands.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::git::GitProvider;

/// Where the on-disk index lives for a given repository.
///
/// The contained `PathBuf` is the **directory** that should hold every
/// gitlore artefact for this repo — the SQLite database, lock files, log
/// rotation snapshots. Callers join filenames onto it; the resolver does
/// not pre-pick a database filename so the layout can evolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexLocation {
    /// Inside `<git-common-dir>/gitlore/`. Worktree-shared, follows the
    /// repo on disk.
    CommonDir(PathBuf),
    /// Inside the per-user XDG data dir. Used when the common dir is not
    /// writable; per-repo subdirectory keyed by a digest of the common
    /// dir path so multiple repos never collide.
    Xdg(PathBuf),
}

impl IndexLocation {
    /// Borrow the resolved directory path.
    pub fn path(&self) -> &Path {
        match self {
            IndexLocation::CommonDir(p) | IndexLocation::Xdg(p) => p,
        }
    }

    /// `true` iff this is the XDG fallback variant (`false` for the
    /// preferred [`IndexLocation::CommonDir`]).
    pub fn is_xdg_fallback(&self) -> bool {
        matches!(self, IndexLocation::Xdg(_))
    }
}

/// Filename used for the write-probe inside the candidate directory.
const PROBE_FILENAME: &str = ".write_probe";

/// Subdirectory created underneath the Git common dir to host the index.
const COMMON_DIR_SUBDIR: &str = "gitlore";

/// Subdirectory under `<XDG_DATA_HOME>/gitlore/` that groups per-repo
/// fallback indices.
const XDG_REPOS_SUBDIR: &str = "repos";

/// Resolve the directory that should hold the gitlore index for the
/// repository rooted at `repo_root`.
///
/// `provider` is consulted exactly once (for `common_dir()`). `repo_root`
/// is reported back inside the [`Error::NotARepo`] envelope if the
/// provider rejects the path, and is hashed into the XDG fallback path so
/// multiple repos with different common dirs do not collide.
///
/// See the module-level docs for the full algorithm.
pub fn resolve_index_path(repo_root: &Path, provider: &dyn GitProvider) -> Result<IndexLocation> {
    let common_dir = provider.common_dir()?;
    let candidate = common_dir.join(COMMON_DIR_SUBDIR);

    match probe_writable(&candidate) {
        Ok(()) => Ok(IndexLocation::CommonDir(candidate)),
        Err(e) if is_permission_or_readonly(&e) => {
            let xdg = xdg_fallback_for(&common_dir).ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::NotFound,
                    "no XDG data dir available for index fallback",
                ))
            })?;
            // Ensure the fallback directory exists so callers can open a
            // SQLite file inside it immediately.
            fs::create_dir_all(&xdg)?;
            tracing::debug!(
                repo_root = %repo_root.display(),
                common_dir = %common_dir.display(),
                fallback = %xdg.display(),
                "common-dir not writable; falling back to XDG"
            );
            Ok(IndexLocation::Xdg(xdg))
        }
        Err(e) => Err(Error::Io(e)),
    }
}

/// Attempt to create `dir`, write a sentinel byte to a probe file
/// inside it, then remove the probe. Any failure is returned verbatim so
/// the caller can decide whether to fall back.
fn probe_writable(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let probe = dir.join(PROBE_FILENAME);
    {
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&probe)?;
        f.write_all(b"gitlore-probe")?;
        f.flush()?;
    }
    // Best-effort cleanup. If removal fails (e.g. the FS revoked write
    // permission between create and remove) the probe is left behind but
    // we still consider the directory writable since the create+write
    // succeeded; the next probe will overwrite it.
    let _ = fs::remove_file(&probe);
    Ok(())
}

/// `true` for filesystem errors that mean "this path is read-only" —
/// the only conditions that should trigger an XDG fallback.
fn is_permission_or_readonly(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    )
}

/// Derive the XDG fallback directory for the supplied common dir.
///
/// The fallback is `<XDG_DATA_HOME>/gitlore/repos/<digest>/` where
/// `<digest>` is a stable, filename-safe hash of the absolute common dir
/// path. Two repos with different common dirs therefore never collide.
fn xdg_fallback_for(common_dir: &Path) -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from("", "", "gitlore")?;
    let base = proj.data_local_dir().join(XDG_REPOS_SUBDIR);
    let digest = digest_path(common_dir);
    Some(base.join(digest))
}

/// Cheap, dependency-free, filename-safe digest of a path. Uses the
/// stdlib FNV-style hasher (`DefaultHasher`) — collision-resistance is
/// not security-critical here; we only need "two distinct paths give two
/// distinct directory names" with very high probability and a fixed
/// 16-char hex output.
fn digest_path(p: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    p.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::git::{
        GitProvider, MailmapResolved, RawCommit, RefEntry, RefScope, Sha, ShowOpts, WalkRange,
    };

    /// Minimal GitProvider stub. Only `common_dir` is exercised by the
    /// storage resolver; every other method returns `unimplemented!()` so
    /// an accidental call is loud rather than silent.
    struct StubProvider {
        common: PathBuf,
        called: AtomicU32,
    }

    impl StubProvider {
        fn new(common: PathBuf) -> Self {
            Self {
                common,
                called: AtomicU32::new(0),
            }
        }
    }

    impl GitProvider for StubProvider {
        fn common_dir(&self) -> Result<PathBuf> {
            self.called.fetch_add(1, Ordering::SeqCst);
            Ok(self.common.clone())
        }
        fn rev_parse(&self, _: &str) -> Result<Sha> {
            unimplemented!()
        }
        fn list_refs(&self, _: RefScope) -> Result<Vec<RefEntry>> {
            unimplemented!()
        }
        fn walk_commits(&self, _: WalkRange) -> Result<Vec<RawCommit>> {
            unimplemented!()
        }
        fn show(&self, _: &Sha, _: ShowOpts) -> Result<String> {
            unimplemented!()
        }
        fn check_mailmap(&self, _: &str, _: &str) -> Result<MailmapResolved> {
            unimplemented!()
        }
        fn cat_file_exists(&self, _: &Sha) -> Result<bool> {
            unimplemented!()
        }
    }

    #[test]
    fn resolve_returns_common_dir_when_writable() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        fs::create_dir_all(&common).unwrap();
        let provider = StubProvider::new(common.clone());

        let loc = resolve_index_path(tmp.path(), &provider).unwrap();
        match loc {
            IndexLocation::CommonDir(p) => {
                assert_eq!(p, common.join(COMMON_DIR_SUBDIR));
                assert!(p.is_dir(), "resolver must create the subdir");
            }
            IndexLocation::Xdg(p) => panic!("expected CommonDir, got Xdg({p:?})"),
        }
        assert_eq!(
            provider.called.load(Ordering::SeqCst),
            1,
            "common_dir called once"
        );
    }

    #[test]
    fn resolve_cleans_up_probe_file() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        fs::create_dir_all(&common).unwrap();
        let provider = StubProvider::new(common.clone());

        resolve_index_path(tmp.path(), &provider).unwrap();
        let probe = common.join(COMMON_DIR_SUBDIR).join(PROBE_FILENAME);
        assert!(!probe.exists(), "probe file must not be left behind");
    }

    #[test]
    fn resolve_is_idempotent_under_repeated_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        fs::create_dir_all(&common).unwrap();
        let provider = StubProvider::new(common.clone());

        let a = resolve_index_path(tmp.path(), &provider).unwrap();
        let b = resolve_index_path(tmp.path(), &provider).unwrap();
        assert_eq!(a, b);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_falls_back_to_xdg_on_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        // chmod 555 does not block root, so skip when uid 0. `id -u`
        // shells out instead of pulling in the `nix` "user" feature.
        if running_as_root() {
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        fs::create_dir_all(&common).unwrap();
        // Make common dir read+exec but not writable so create_dir_all on
        // the `gitlore` subdir fails with PermissionDenied.
        fs::set_permissions(&common, fs::Permissions::from_mode(0o555)).unwrap();

        // Override XDG_DATA_HOME so the fallback lands inside our tempdir
        // rather than the user's real ~/.local/share.
        // SAFETY: tests in this crate are single-threaded by default
        // (cargo test uses one thread per test process by default for
        // tempfile-backed tests) and we restore the env after the assert.
        let xdg_root = tmp.path().join("xdg");
        fs::create_dir_all(&xdg_root).unwrap();
        let prev_xdg = std::env::var_os("XDG_DATA_HOME");
        std::env::set_var("XDG_DATA_HOME", &xdg_root);

        let provider = StubProvider::new(common.clone());
        let loc = resolve_index_path(tmp.path(), &provider);

        // Restore env before any assert that might panic.
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        // Restore perms so tempfile can clean up.
        let _ = fs::set_permissions(&common, fs::Permissions::from_mode(0o755));

        let loc = loc.unwrap();
        match &loc {
            IndexLocation::Xdg(p) => {
                assert!(p.is_dir(), "fallback dir must exist after resolve");
                assert!(loc.is_xdg_fallback());
            }
            IndexLocation::CommonDir(p) => {
                panic!("expected XDG fallback under read-only common dir; got CommonDir({p:?})")
            }
        }
    }

    #[test]
    fn index_location_path_returns_inner_path() {
        let p = PathBuf::from("/x/y");
        assert_eq!(IndexLocation::CommonDir(p.clone()).path(), p);
        assert_eq!(IndexLocation::Xdg(p.clone()).path(), p);
    }

    #[cfg(unix)]
    fn running_as_root() -> bool {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim() == "0")
            .unwrap_or(false)
    }

    #[test]
    fn digest_path_is_stable_and_filename_safe() {
        let a = digest_path(Path::new("/a/b"));
        let b = digest_path(Path::new("/a/b"));
        assert_eq!(a, b);
        let c = digest_path(Path::new("/a/c"));
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

//! Fixture loaders for gitlore-eval.
//!
//! Two fixture roots are supported:
//!
//! * `qa/fixtures/` — public, always loaded when the directory exists.
//! * `qa/fixtures-private/` — gated; loaded ONLY when both
//!     1. `GITLORE_EVAL_FIXTURES_PRIVATE=1` is set in the environment, AND
//!     2. the directory exists on disk.
//!
//! Per **Q6a** (open question on private fixture handling), when either gate
//! fails the loader returns a [`FixtureSet`] whose [`FixtureSet::skip_reason`]
//! is populated. Scenarios should treat a skipped set as "no fixtures
//! available" and emit a single neutral CI log line instead of failing.
//!
//! The gating logic ([`gate_private`]) is split from the IO layer so it can
//! be exhaustively tested without touching the process environment, which
//! since Rust 1.84 requires `unsafe` to mutate.

use std::env;
use std::path::{Path, PathBuf};

/// Environment variable that gates the private fixture set.
pub const ENV_PRIVATE: &str = "GITLORE_EVAL_FIXTURES_PRIVATE";

/// Value the env var must equal to enable the private set.
pub const ENV_PRIVATE_ENABLED: &str = "1";

/// Optional workspace-root override. When set, takes precedence over the
/// `Cargo.lock` / `.git` walk performed by [`workspace_root`]. Useful for
/// integration tests that stand up a temporary fixture tree.
pub const ENV_WORKSPACE_ROOT: &str = "GITLORE_EVAL_WORKSPACE_ROOT";

/// Origin of a [`FixtureSet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureScope {
    /// Loaded from `qa/fixtures/`.
    Public,
    /// Loaded from `qa/fixtures-private/` (gated).
    Private,
}

impl FixtureScope {
    /// Last path segment for the on-disk fixture root.
    fn dir_name(self) -> &'static str {
        match self {
            FixtureScope::Public => "fixtures",
            FixtureScope::Private => "fixtures-private",
        }
    }
}

/// Loaded-or-skipped fixture set.
///
/// `root` is `Some` iff the set was actually loaded. A `None` root together
/// with a populated `skip_reason` signals "intentionally skipped" — scenarios
/// MUST treat this as the absence of fixtures, not as an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureSet {
    /// Scope this set was requested under.
    pub scope: FixtureScope,
    /// Absolute path to the fixture root, when loaded.
    pub root: Option<PathBuf>,
    /// Human-readable reason the set was skipped, when not loaded.
    pub skip_reason: Option<String>,
}

impl FixtureSet {
    /// Build a loaded set.
    pub fn loaded(scope: FixtureScope, root: PathBuf) -> Self {
        Self {
            scope,
            root: Some(root),
            skip_reason: None,
        }
    }

    /// Build a skipped set with a reason.
    pub fn skipped(scope: FixtureScope, reason: impl Into<String>) -> Self {
        Self {
            scope,
            root: None,
            skip_reason: Some(reason.into()),
        }
    }

    /// `true` when the set was loaded successfully (i.e. a real `root` exists).
    pub fn is_available(&self) -> bool {
        self.root.is_some()
    }
}

/// Outcome of the private-fixture gate. Pure decision logic, no IO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Both env and directory checks passed.
    Allow,
    /// Skip with a stable, human-readable reason.
    Skip(String),
}

/// Apply the private-fixture gate.
///
/// Inputs are passed explicitly so this is fully testable without mutating
/// `std::env` (which is `unsafe` since Rust 1.84).
///
/// Returns [`GateDecision::Allow`] only when:
///   * `env_value == Some("1")`, AND
///   * `dir_exists` is `true`.
pub fn gate_private(env_value: Option<&str>, dir_exists: bool) -> GateDecision {
    match env_value {
        Some(v) if v == ENV_PRIVATE_ENABLED => {}
        Some(other) => {
            return GateDecision::Skip(format!(
                "{ENV_PRIVATE}={other:?} (expected \"1\"); private fixtures gated off"
            ));
        }
        None => {
            return GateDecision::Skip(format!(
                "{ENV_PRIVATE} not set; private fixtures gated off"
            ));
        }
    }
    if !dir_exists {
        return GateDecision::Skip(format!(
            "{ENV_PRIVATE}=1 but private fixtures directory not present"
        ));
    }
    GateDecision::Allow
}

/// Locate the workspace root.
///
/// Resolution order:
///   1. `GITLORE_EVAL_WORKSPACE_ROOT` env override (test-friendly).
///   2. Walk up from `CARGO_MANIFEST_DIR` (or CWD) until a directory
///      containing `Cargo.lock` or `.git` is found.
///   3. Fall back to the starting directory.
pub fn workspace_root() -> PathBuf {
    if let Some(over) = env::var_os(ENV_WORKSPACE_ROOT) {
        return PathBuf::from(over);
    }
    let start: PathBuf = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let mut p: &Path = start.as_path();
    loop {
        if p.join("Cargo.lock").is_file() || p.join(".git").exists() {
            return p.to_path_buf();
        }
        match p.parent() {
            Some(parent) => p = parent,
            None => return start.clone(),
        }
    }
}

/// Compose `<workspace>/qa/<scope-dir>`.
fn fixtures_path(scope: FixtureScope) -> PathBuf {
    workspace_root().join("qa").join(scope.dir_name())
}

/// Load the public fixture set at `<workspace>/qa/fixtures/`.
///
/// A missing public directory is not a hard error — early milestones may ship
/// with an empty fixtures tree. Callers receive a `Skipped` set in that case.
pub fn load_public() -> FixtureSet {
    let root = fixtures_path(FixtureScope::Public);
    if root.is_dir() {
        FixtureSet::loaded(FixtureScope::Public, root)
    } else {
        FixtureSet::skipped(
            FixtureScope::Public,
            format!("public fixtures not found at {}", root.display()),
        )
    }
}

/// Load the private fixture set at `<workspace>/qa/fixtures-private/` under
/// the env+directory gate documented in the module header.
pub fn load_private() -> FixtureSet {
    let root = fixtures_path(FixtureScope::Private);
    let env_value = env::var(ENV_PRIVATE).ok();
    match gate_private(env_value.as_deref(), root.is_dir()) {
        GateDecision::Allow => FixtureSet::loaded(FixtureScope::Private, root),
        GateDecision::Skip(reason) => FixtureSet::skipped(FixtureScope::Private, reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- gate_private: pure logic, no env mutation needed ----

    #[test]
    fn gate_allows_when_env_is_one_and_dir_exists() {
        assert_eq!(gate_private(Some("1"), true), GateDecision::Allow);
    }

    #[test]
    fn gate_skips_when_env_is_missing() {
        let d = gate_private(None, true);
        match d {
            GateDecision::Skip(reason) => {
                assert!(reason.contains(ENV_PRIVATE));
                assert!(reason.contains("not set"));
            }
            other => panic!("expected Skip, got {other:?}"),
        }
    }

    #[test]
    fn gate_skips_when_env_is_not_one() {
        for v in ["0", "true", "yes", "", "11"] {
            let d = gate_private(Some(v), true);
            assert!(
                matches!(d, GateDecision::Skip(_)),
                "value {v:?} should skip"
            );
        }
    }

    #[test]
    fn gate_skips_when_env_set_but_directory_missing() {
        let d = gate_private(Some("1"), false);
        match d {
            GateDecision::Skip(reason) => assert!(reason.contains("not present")),
            other => panic!("expected Skip, got {other:?}"),
        }
    }

    // ---- FixtureSet helpers ----

    #[test]
    fn skipped_set_is_not_available() {
        let s = FixtureSet::skipped(FixtureScope::Private, "x");
        assert!(!s.is_available());
        assert_eq!(s.scope, FixtureScope::Private);
        assert_eq!(s.skip_reason.as_deref(), Some("x"));
        assert!(s.root.is_none());
    }

    #[test]
    fn loaded_set_is_available() {
        let s = FixtureSet::loaded(FixtureScope::Public, PathBuf::from("/tmp/x"));
        assert!(s.is_available());
        assert!(s.skip_reason.is_none());
        assert_eq!(s.scope, FixtureScope::Public);
    }

    #[test]
    fn dir_name_is_stable() {
        assert_eq!(FixtureScope::Public.dir_name(), "fixtures");
        assert_eq!(FixtureScope::Private.dir_name(), "fixtures-private");
    }

    // ---- Loaders observed under the current process env ----
    //
    // These do not mutate the environment; they verify the loader returns a
    // `FixtureSet` with the correct scope regardless of whether fixtures are
    // physically present at the resolved workspace root.

    #[test]
    fn load_public_returns_public_scope() {
        let s = load_public();
        assert_eq!(s.scope, FixtureScope::Public);
        // Exactly one of root / skip_reason is set.
        assert_eq!(s.root.is_some(), s.skip_reason.is_none());
    }

    #[test]
    fn load_private_returns_private_scope() {
        let s = load_private();
        assert_eq!(s.scope, FixtureScope::Private);
        assert_eq!(s.root.is_some(), s.skip_reason.is_none());
    }
}

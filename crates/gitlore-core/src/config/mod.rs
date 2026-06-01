//! User-facing configuration for gitlore.
//!
//! All defaults are encoded in this module via `serde(default)` so that an
//! empty (or missing) TOML file still produces a fully-populated `Config`.
//!
//! ## Layering
//!
//! [`Config::load`] reads up to two files and merges them with **per-repo
//! wins** semantics:
//!
//! 1. `~/.config/gitlore/config.toml` — user-global (XDG-aware via
//!    `$XDG_CONFIG_HOME`)
//! 2. `<git-common-dir>/gitlore/config.toml` — per-repo override
//!
//! Merging is a deep merge at the TOML-value level: any key present in the
//! per-repo file replaces the same key from user-global; sibling keys from
//! user-global remain. Tables are merged recursively; arrays and scalars are
//! replaced wholesale. Missing files are not errors; the resolved value is
//! always at least [`Config::default`].
//!
//! ## Numeric defaults
//!
//! All defaults are documented in §15 of `gitlore_unified_spec.md` and on the
//! corresponding `impl Default for ...` blocks below. Risk weights sum to
//! 1.00 by construction.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Top-level Config
// ---------------------------------------------------------------------------

/// Complete user-facing configuration. Every field has a default, so the
/// `Default` impl produces the canonical "out of the box" config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub index: IndexConfig,
    pub search: SearchConfig,
    pub story: StoryConfig,
    pub risk: RiskConfig,
    pub ownership: OwnershipConfig,
    pub tui: TuiConfig,
    pub classification: ClassificationConfig,
}

impl Config {
    /// Resolve configuration by layering user-global under per-repo overrides.
    ///
    /// `git_common_dir` is the Git **common directory** (the directory that
    /// contains `HEAD`, `refs/`, `objects/`, etc.). For a normal repo this is
    /// `.git`; for a linked worktree it is the parent repo's `.git` directory.
    /// Per-repo config lives at `<git-common-dir>/gitlore/config.toml`.
    ///
    /// Missing files are silently treated as empty. Present-but-unparseable
    /// files surface [`ConfigError::Parse`] tagged with the offending path.
    pub fn load(git_common_dir: &Path) -> Result<Self, ConfigError> {
        let user_path = user_global_config_path();
        let repo_path = git_common_dir.join("gitlore").join("config.toml");
        Self::load_from_paths(user_path.as_deref(), Some(&repo_path))
    }

    /// Variant of [`Self::load`] that takes both paths explicitly. Exposed
    /// for testing and for callers that want to opt out of the XDG lookup.
    pub fn load_from_paths(
        user_global: Option<&Path>,
        per_repo: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let user_value = match user_global {
            Some(p) => read_toml_value(p)?,
            None => None,
        };
        let repo_value = match per_repo {
            Some(p) => read_toml_value(p)?,
            None => None,
        };

        let merged = match (user_value, repo_value) {
            (None, None) => return Ok(Self::default()),
            (Some(u), None) => u,
            (None, Some(r)) => r,
            (Some(u), Some(r)) => deep_merge(u, r),
        };

        merged.try_into().map_err(ConfigError::Deserialize)
    }
}

// ---------------------------------------------------------------------------
// IndexConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexConfig {
    /// Include `refs/remotes/**` heads when walking commits. Default: `true`.
    pub include_remote_refs: bool,
    /// Include annotated and lightweight tags as walk roots. Default: `true`.
    pub include_tags: bool,
    /// Hard cap on the number of commits ingested by the initial index pass.
    /// Default: `50_000` (matches the monorepo-safety bound in §22).
    pub max_initial_commits: u64,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            include_remote_refs: true,
            include_tags: true,
            max_initial_commits: 50_000,
        }
    }
}

// ---------------------------------------------------------------------------
// SearchConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    /// Half-life (in days) used in the recency-decay term of the ranking
    /// blend. Default: `180`.
    pub recency_half_life_days: u32,
    /// Per-field BM25 weights applied during lexical scoring.
    pub bm25_weights: Bm25Weights,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            recency_half_life_days: 180,
            bm25_weights: Bm25Weights::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bm25Weights {
    /// Weight applied to the subject line. Default: `4.0`.
    pub subject: f64,
    /// Weight applied to the commit body. Default: `1.0`.
    pub body: f64,
    /// Weight applied to expanded/synonym fields (e.g. ref names). Default: `1.5`.
    pub expanded: f64,
    /// Weight applied to touched paths. Default: `2.0`.
    pub paths: f64,
}

impl Default for Bm25Weights {
    fn default() -> Self {
        Self {
            subject: 4.0,
            body: 1.0,
            expanded: 1.5,
            paths: 2.0,
        }
    }
}

// ---------------------------------------------------------------------------
// StoryConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StoryConfig {
    /// Minimum cluster-coherence score for attaching a commit to an existing
    /// story. Default: `0.5`.
    pub coherence_threshold: f64,
    /// Maximum gap, in hours, between adjacent commits in the same story.
    /// Default: `24`.
    pub time_window_hours: u32,
    /// If `true`, only commits whose author overlaps an existing story may
    /// attach to it. Default: `false`.
    pub require_author_overlap: bool,
}

impl Default for StoryConfig {
    fn default() -> Self {
        Self {
            coherence_threshold: 0.5,
            time_window_hours: 24,
            require_author_overlap: false,
        }
    }
}

// ---------------------------------------------------------------------------
// RiskConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RiskConfig {
    pub weights: RiskWeights,
    pub label_cutoffs: RiskLabelCutoffs,
}

/// Per-factor weights for the additive risk score (§15). Defaults sum to 1.0:
/// `0.20 + 0.15 + 0.20 + 0.15 + 0.15 + 0.10 + 0.05 = 1.00`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RiskWeights {
    pub files: f64,
    pub dirs: f64,
    pub infra: f64,
    pub tests: f64,
    pub churn: f64,
    pub revert: f64,
    pub release: f64,
}

impl Default for RiskWeights {
    fn default() -> Self {
        Self {
            files: 0.20,
            dirs: 0.15,
            infra: 0.20,
            tests: 0.15,
            churn: 0.15,
            revert: 0.10,
            release: 0.05,
        }
    }
}

/// Score cutoffs that bucket a normalized risk in `[0.0, 1.0]` into the UI
/// labels `low` / `medium` / `high`. A score `s` is labelled:
///
/// * `low`    if `s < medium`
/// * `medium` if `medium <= s < high`
/// * `high`   if `s >= high`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RiskLabelCutoffs {
    pub medium: f64,
    pub high: f64,
}

impl Default for RiskLabelCutoffs {
    fn default() -> Self {
        Self {
            medium: 0.34,
            high: 0.67,
        }
    }
}

// ---------------------------------------------------------------------------
// OwnershipConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OwnershipConfig {
    /// Half-life (in days) used to decay older commits in the
    /// frequency-plus-recency ownership signal. Default: `365`.
    pub recency_half_life_days: u32,
    /// Fractional credit assigned to co-authors (`Co-authored-by:` trailers)
    /// relative to the primary author. Default: `0.5`.
    pub coauthor_weight: f64,
    /// How many owners to surface per path. Default: `5`.
    pub top_n: u32,
    /// Include known automation accounts (bots) in ownership tallies.
    /// Default: `false`. The bot list lives in
    /// [`ClassificationConfig::bot_authors`].
    pub include_bots: bool,
}

impl Default for OwnershipConfig {
    fn default() -> Self {
        Self {
            recency_half_life_days: 365,
            coauthor_weight: 0.5,
            top_n: 5,
            include_bots: false,
        }
    }
}

// ---------------------------------------------------------------------------
// TuiConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    /// Theme preference. Default: [`Theme::Auto`] (follow terminal background
    /// when detectable).
    pub theme: Theme,
    /// If `true`, swap chromatic palettes for color-vision-deficiency-safe
    /// ones. Default: `false`.
    pub color_blind_safe: bool,
    /// Capture mouse events. Default: `false` (keyboard-first per §11.9).
    pub mouse: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: Theme::Auto,
            color_blind_safe: false,
            mouse: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

// ---------------------------------------------------------------------------
// ClassificationConfig
// ---------------------------------------------------------------------------

/// Globs and known-author lists used to classify files (test / config /
/// infra) and authors (bot vs human). Used by the risk scorer (§15) and the
/// ownership scorer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClassificationConfig {
    /// Glob patterns that mark a path as a test file.
    pub test_globs: Vec<String>,
    /// Glob patterns that mark a path as a configuration file.
    pub config_globs: Vec<String>,
    /// Glob patterns that mark a path as infrastructure (CI, IaC, container
    /// orchestration, etc.).
    pub infra_globs: Vec<String>,
    /// Author identifiers (typically in `name <email>` form, or the GitHub
    /// `*[bot]` convention) that are treated as automation.
    pub bot_authors: Vec<String>,
}

impl Default for ClassificationConfig {
    fn default() -> Self {
        Self {
            test_globs: vec![
                "**/tests/**".into(),
                "**/test/**".into(),
                "**/__tests__/**".into(),
                "**/*_test.go".into(),
                "**/*_test.rs".into(),
                "**/test_*.py".into(),
                "**/*.test.js".into(),
                "**/*.test.ts".into(),
                "**/*.test.jsx".into(),
                "**/*.test.tsx".into(),
                "**/*.spec.js".into(),
                "**/*.spec.ts".into(),
            ],
            config_globs: vec![
                "**/*.toml".into(),
                "**/*.yaml".into(),
                "**/*.yml".into(),
                "**/*.json".into(),
                "**/*.ini".into(),
                "**/.env".into(),
                "**/.env.*".into(),
            ],
            infra_globs: vec![
                "**/Dockerfile".into(),
                "**/Dockerfile.*".into(),
                "**/docker-compose*.yml".into(),
                "**/docker-compose*.yaml".into(),
                "**/k8s/**".into(),
                "**/kubernetes/**".into(),
                "**/helm/**".into(),
                "**/.github/workflows/**".into(),
                "**/.gitlab-ci.yml".into(),
                "**/*.tf".into(),
                "**/terraform/**".into(),
            ],
            bot_authors: vec![
                "dependabot[bot]".into(),
                "renovate[bot]".into(),
                "github-actions[bot]".into(),
            ],
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Error type for configuration loading. Once `crate::error::Error` lands,
/// these variants will be wrapped via `#[from]` so callers can treat config
/// failures as ordinary core errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// I/O error reading a config file (other than `NotFound`, which is
    /// silently treated as "file absent").
    #[error("io error reading config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// TOML syntax error in the file at `path`.
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    /// TOML parsed cleanly but did not match the `Config` schema (unknown
    /// key, wrong type, etc.). Maps to error codes `ConfigInvalidKey` and
    /// `ConfigTypeMismatch` from SPEC-001 §4.3.
    #[error("config does not match schema: {0}")]
    Deserialize(#[source] toml::de::Error),
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Resolve `~/.config/gitlore/config.toml`, honoring `$XDG_CONFIG_HOME` when
/// set. Returns `None` when neither `$XDG_CONFIG_HOME` nor `$HOME` is set,
/// which is the case in some minimal sandboxes.
fn user_global_config_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("gitlore").join("config.toml"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("gitlore")
            .join("config.toml"),
    )
}

/// Read a TOML file and return its parsed `toml::Value`. A missing file is
/// not an error (returns `Ok(None)`); other I/O failures and parse failures
/// are surfaced with the offending path attached.
fn read_toml_value(path: &Path) -> Result<Option<toml::Value>, ConfigError> {
    let contents = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let value = contents
        .parse::<toml::Value>()
        .map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(Some(value))
}

/// Recursively merge `override_` into `base`. Tables are merged key by key;
/// any non-table value (scalar, array, datetime) in `override_` wholesale
/// replaces the value in `base`. Keys present only in `base` are preserved.
fn deep_merge(base: toml::Value, override_: toml::Value) -> toml::Value {
    match (base, override_) {
        (toml::Value::Table(mut b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.remove(&k) {
                    Some(existing) => {
                        b.insert(k, deep_merge(existing, v));
                    }
                    None => {
                        b.insert(k, v);
                    }
                }
            }
            toml::Value::Table(b)
        }
        // Scalars, arrays, and type-mismatched tables: override wins.
        (_, v) => v,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_defaults_match_spec(cfg: &Config) {
        // IndexConfig
        assert!(cfg.index.include_remote_refs);
        assert!(cfg.index.include_tags);
        assert_eq!(cfg.index.max_initial_commits, 50_000);

        // SearchConfig
        assert_eq!(cfg.search.recency_half_life_days, 180);
        assert_eq!(cfg.search.bm25_weights.subject, 4.0);
        assert_eq!(cfg.search.bm25_weights.body, 1.0);
        assert_eq!(cfg.search.bm25_weights.expanded, 1.5);
        assert_eq!(cfg.search.bm25_weights.paths, 2.0);

        // StoryConfig
        assert_eq!(cfg.story.coherence_threshold, 0.5);
        assert_eq!(cfg.story.time_window_hours, 24);
        assert!(!cfg.story.require_author_overlap);

        // RiskConfig
        let w = &cfg.risk.weights;
        assert_eq!(w.files, 0.20);
        assert_eq!(w.dirs, 0.15);
        assert_eq!(w.infra, 0.20);
        assert_eq!(w.tests, 0.15);
        assert_eq!(w.churn, 0.15);
        assert_eq!(w.revert, 0.10);
        assert_eq!(w.release, 0.05);

        // Risk weights sum to 1.0 (within FP tolerance).
        let sum = w.files + w.dirs + w.infra + w.tests + w.churn + w.revert + w.release;
        assert!(
            (sum - 1.0).abs() < 1e-9,
            "risk weights must sum to 1.0; got {sum}"
        );

        // OwnershipConfig
        assert_eq!(cfg.ownership.recency_half_life_days, 365);
        assert_eq!(cfg.ownership.coauthor_weight, 0.5);
        assert_eq!(cfg.ownership.top_n, 5);
        assert!(!cfg.ownership.include_bots);

        // TuiConfig
        assert_eq!(cfg.tui.theme, Theme::Auto);
        assert!(!cfg.tui.color_blind_safe);
        assert!(!cfg.tui.mouse);

        // ClassificationConfig: nontrivial defaults
        assert!(!cfg.classification.test_globs.is_empty());
        assert!(!cfg.classification.config_globs.is_empty());
        assert!(!cfg.classification.infra_globs.is_empty());
        assert!(!cfg.classification.bot_authors.is_empty());
    }

    #[test]
    fn default_matches_spec() {
        assert_defaults_match_spec(&Config::default());
    }

    #[test]
    fn empty_toml_yields_defaults() {
        let cfg: Config = toml::from_str("").expect("empty TOML must parse as Config");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn partial_toml_overrides_only_named_fields() {
        let cfg: Config = toml::from_str(
            r#"
[index]
max_initial_commits = 1000

[search.bm25_weights]
subject = 7.5
"#,
        )
        .expect("partial TOML must parse");

        assert_eq!(cfg.index.max_initial_commits, 1000);
        // Sibling Index keys keep their defaults.
        assert!(cfg.index.include_remote_refs);
        assert!(cfg.index.include_tags);

        assert_eq!(cfg.search.bm25_weights.subject, 7.5);
        // Sibling BM25 keys keep their defaults.
        assert_eq!(cfg.search.bm25_weights.body, 1.0);
        assert_eq!(cfg.search.bm25_weights.expanded, 1.5);
        assert_eq!(cfg.search.bm25_weights.paths, 2.0);
        // Sibling Search keys keep their defaults.
        assert_eq!(cfg.search.recency_half_life_days, 180);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let err = toml::from_str::<Config>(
            r#"
[index]
bogus = true
"#,
        )
        .expect_err("unknown key must fail with deny_unknown_fields");
        let msg = err.to_string();
        assert!(
            msg.contains("bogus"),
            "error should name the bogus key: {msg}"
        );
    }

    #[test]
    fn theme_round_trip_lowercase() {
        let cfg: Config = toml::from_str(
            r#"
[tui]
theme = "dark"
"#,
        )
        .expect("dark theme must parse");
        assert_eq!(cfg.tui.theme, Theme::Dark);

        let s = toml::to_string(&cfg).expect("serialize");
        assert!(
            s.contains("theme = \"dark\""),
            "theme must serialize lowercase: {s}"
        );
    }

    #[test]
    fn round_trip_preserves_defaults() {
        let original = Config::default();
        let serialized = toml::to_string(&original).expect("serialize defaults");
        let parsed: Config = toml::from_str(&serialized).expect("parse serialized defaults");
        assert_eq!(parsed, original);
    }

    #[test]
    fn load_from_paths_missing_files_returns_defaults() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing_user = tmp.path().join("user.toml");
        let missing_repo = tmp.path().join("repo.toml");

        let cfg = Config::load_from_paths(Some(&missing_user), Some(&missing_repo))
            .expect("missing files must not error");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn load_from_paths_per_repo_overrides_user_global() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user.toml");
        let repo = tmp.path().join("repo.toml");

        fs::write(
            &user,
            r#"
[index]
max_initial_commits = 100
include_tags = false

[search]
recency_half_life_days = 30
"#,
        )
        .unwrap();

        fs::write(
            &repo,
            r#"
[index]
max_initial_commits = 200

[story]
time_window_hours = 6
"#,
        )
        .unwrap();

        let cfg = Config::load_from_paths(Some(&user), Some(&repo)).expect("load merged");

        // Per-repo wins where it speaks.
        assert_eq!(cfg.index.max_initial_commits, 200);
        // User-global survives where per-repo is silent.
        assert!(!cfg.index.include_tags);
        assert_eq!(cfg.search.recency_half_life_days, 30);
        // Per-repo's new key takes effect.
        assert_eq!(cfg.story.time_window_hours, 6);
        // Untouched defaults stay default.
        assert!(cfg.index.include_remote_refs);
    }

    #[test]
    fn load_from_paths_only_user_global() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user.toml");
        let missing_repo = tmp.path().join("repo.toml");

        fs::write(&user, "[tui]\ntheme = \"light\"\n").unwrap();

        let cfg =
            Config::load_from_paths(Some(&user), Some(&missing_repo)).expect("load user only");
        assert_eq!(cfg.tui.theme, Theme::Light);
    }

    #[test]
    fn load_from_paths_only_per_repo() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing_user = tmp.path().join("user.toml");
        let repo = tmp.path().join("repo.toml");

        fs::write(&repo, "[tui]\nmouse = true\n").unwrap();

        let cfg =
            Config::load_from_paths(Some(&missing_user), Some(&repo)).expect("load repo only");
        assert!(cfg.tui.mouse);
    }

    #[test]
    fn malformed_toml_surfaces_path_in_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user.toml");
        fs::write(&user, "this is = = not valid toml").unwrap();

        let err = Config::load_from_paths(Some(&user), None).expect_err("must reject bad toml");
        match err {
            ConfigError::Parse { path, .. } => assert_eq!(path, user),
            other => panic!("expected Parse error, got: {other:?}"),
        }
    }

    #[test]
    fn schema_mismatch_surfaces_as_deserialize_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user.toml");
        fs::write(
            &user,
            r#"
[index]
max_initial_commits = "not-a-number"
"#,
        )
        .unwrap();

        let err = Config::load_from_paths(Some(&user), None).expect_err("must reject bad type");
        assert!(matches!(err, ConfigError::Deserialize(_)));
    }

    #[test]
    fn deep_merge_replaces_arrays_wholesale() {
        // Arrays are NOT element-wise merged; per-repo replaces user-global.
        let tmp = tempfile::tempdir().expect("tempdir");
        let user = tmp.path().join("user.toml");
        let repo = tmp.path().join("repo.toml");

        fs::write(
            &user,
            r#"
[classification]
bot_authors = ["bot-a", "bot-b"]
"#,
        )
        .unwrap();
        fs::write(
            &repo,
            r#"
[classification]
bot_authors = ["bot-c"]
"#,
        )
        .unwrap();

        let cfg = Config::load_from_paths(Some(&user), Some(&repo)).expect("load");
        assert_eq!(cfg.classification.bot_authors, vec!["bot-c".to_string()]);
    }

    #[test]
    fn load_uses_git_common_dir_subpath() {
        // End-to-end smoke for the public `load`: per-repo file is read from
        // `<common>/gitlore/config.toml`. We don't exercise the user-global
        // path here because it depends on env vars; that's covered by
        // `load_from_paths_*` tests.
        let tmp = tempfile::tempdir().expect("tempdir");
        let common = tmp.path();
        let gitlore_dir = common.join("gitlore");
        fs::create_dir_all(&gitlore_dir).unwrap();
        fs::write(
            gitlore_dir.join("config.toml"),
            "[story]\ncoherence_threshold = 0.9\n",
        )
        .unwrap();

        // Point XDG at an empty dir so user-global is silent.
        let xdg = tmp.path().join("xdg");
        fs::create_dir_all(&xdg).unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_home = std::env::var_os("HOME");
        // SAFETY: tests in this module are not `#[ignore]`d; cargo runs them
        // in parallel by default. To stay safe we serialize on a mutex.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
        std::env::set_var("HOME", tmp.path());

        let cfg = Config::load(common).expect("load");
        assert_eq!(cfg.story.coherence_threshold, 0.9);
        // Other story fields keep defaults.
        assert_eq!(cfg.story.time_window_hours, 24);

        // Restore env.
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    // Env-mutating tests serialize through this mutex so parallel test
    // execution doesn't race on process-global env vars.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}

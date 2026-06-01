//! File classification (M3-5, TDD-000 §2.2, SPEC-001 §4.4, ADR-018, Q14).
//!
//! Given a repo-relative path, produce a single [`Category`]. The
//! [`Classifier`] is built once at indexer start, then reused for every
//! file in every commit; `classify` is the hot path so the per-category
//! glob sets are stored as `globset::GlobSet` so a path is matched
//! against an entire category in one batched call.
//!
//! ## Precedence (Q14)
//!
//! Categories evaluate in fixed order, first-matching-category-wins:
//!
//! ```text
//! generated > migration > test > infra > ci > config > asset > docs > code
//! ```
//!
//! Note that this is *category-level* precedence, not glob-level. If a
//! file matches both a `test` glob and a `migration` glob the result is
//! [`Category::Migration`] because the migration set is evaluated first.
//! Paths that match nothing fall through to [`Category::Code`] — code is
//! the implicit default.
//!
//! ## Defaults + ecosystem overlays
//!
//! [`Classifier::default_for`] reads
//! `defaults.classifications.toml` (embedded via `include_str!`) and
//! then walks `repo_root` for ecosystem marker files (e.g.
//! `Cargo.toml`, `package.json`, `go.mod`). When a marker is detected
//! the corresponding `[ecosystem.<name>]` table is appended to the
//! per-category glob sets. Detected ecosystems are exposed via
//! [`Classifier::ecosystems`] so the indexer can surface them in logs.

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

use crate::error::{Error, Result};

/// The nine canonical file categories per Q14 precedence (TDD-000 §2.2).
///
/// Variants are listed in their precedence order so iterating
/// `ORDERED_CATEGORIES` walks the chain top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Generated artefacts (protobuf output, vendored deps, `dist/`,
    /// `target/`, `node_modules/`). Highest precedence so that touching
    /// generated output never inflates real-code change counts.
    Generated,
    /// Database / schema migrations.
    Migration,
    /// Test sources and harness code.
    Test,
    /// Deployment + infra-as-code (Dockerfiles, Terraform, k8s,
    /// Pulumi).
    Infra,
    /// Continuous-integration configuration
    /// (`.github/workflows/`, `.circleci/`, `Jenkinsfile`).
    Ci,
    /// Repo configuration (TOML / YAML / JSON / dotfiles /
    /// `.properties`).
    Config,
    /// Binary or media assets (images, fonts, audio/video).
    Asset,
    /// Documentation (Markdown, RST, plain text, `docs/`, top-level
    /// `README`/`CHANGELOG`/`LICENSE`).
    Docs,
    /// Source code. The implicit default when no other category
    /// matches.
    Code,
}

impl Category {
    /// Stable kebab-case identifier (for logs / JSON output).
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Generated => "generated",
            Category::Migration => "migration",
            Category::Test => "test",
            Category::Infra => "infra",
            Category::Ci => "ci",
            Category::Config => "config",
            Category::Asset => "asset",
            Category::Docs => "docs",
            Category::Code => "code",
        }
    }
}

/// Q14 precedence chain. The order is load-bearing — [`Classifier::classify`]
/// walks this slice top to bottom and returns the first matching category.
pub const ORDERED_CATEGORIES: [Category; 9] = [
    Category::Generated,
    Category::Migration,
    Category::Test,
    Category::Infra,
    Category::Ci,
    Category::Config,
    Category::Asset,
    Category::Docs,
    Category::Code,
];

/// Path-based file classifier built from default + ecosystem-specific
/// globs (M3-5).
///
/// One [`GlobSet`] per category. `classify` does at most nine batched
/// matches — one per category — and returns the first hit.
#[derive(Debug)]
pub struct Classifier {
    /// Per-category compiled glob sets, in Q14 precedence order so the
    /// matching loop can iterate by index.
    sets: [GlobSet; 9],
    /// Names of the ecosystem overlays that were detected at
    /// `repo_root` (sorted ascending). Exposed for diagnostics; the
    /// underlying globs are already merged into `sets`.
    ecosystems: Vec<String>,
}

impl Classifier {
    /// Build a classifier from the embedded
    /// `defaults.classifications.toml`, augmented with whichever
    /// ecosystem overlays apply at `repo_root`.
    ///
    /// `repo_root` is scanned shallowly: only repo-root-relative
    /// `marker` filenames listed in each `[ecosystem.*]` table are
    /// probed via `path.join(marker).exists()`. Subdirectory scans are
    /// out of scope on purpose — markers in nested workspaces should be
    /// added explicitly.
    ///
    /// # Errors
    ///
    /// * [`Error::Io`] if the embedded TOML cannot be parsed
    ///   (re-encoded as a wrapped [`std::io::Error`] of kind
    ///   `InvalidData`).
    pub fn default_for(repo_root: &Path) -> Result<Self> {
        let raw = include_str!("defaults.classifications.toml");
        let cfg: ClassificationsToml = toml::from_str(raw).map_err(|e| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("defaults.classifications.toml is malformed: {e}"),
            ))
        })?;

        // Per-category accumulators in Q14 order.
        let mut per_category: [Vec<String>; 9] = Default::default();
        merge_into(&mut per_category, &cfg.default);

        let mut detected: Vec<String> = Vec::new();
        for (name, eco) in &cfg.ecosystem {
            if eco.marker.iter().any(|m| repo_root.join(m).exists()) {
                merge_into(&mut per_category, &eco.globs);
                detected.push(name.clone());
            }
        }
        detected.sort();

        let mut sets: [Option<GlobSet>; 9] = Default::default();
        for (idx, patterns) in per_category.iter().enumerate() {
            let mut builder = GlobSetBuilder::new();
            for p in patterns {
                let glob = Glob::new(p).map_err(|e| {
                    Error::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "invalid glob `{p}` for category {}: {e}",
                            ORDERED_CATEGORIES[idx].as_str()
                        ),
                    ))
                })?;
                builder.add(glob);
            }
            let built = builder.build().map_err(|e| {
                Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "could not compile glob set for category {}: {e}",
                        ORDERED_CATEGORIES[idx].as_str()
                    ),
                ))
            })?;
            sets[idx] = Some(built);
        }

        Ok(Classifier {
            sets: sets.map(|s| s.expect("every category slot populated above")),
            ecosystems: detected,
        })
    }

    /// Names of ecosystem overlays that were detected at construction
    /// (sorted ascending).
    pub fn ecosystems(&self) -> &[String] {
        &self.ecosystems
    }

    /// Classify a single repo-relative path.
    ///
    /// Returns [`Category::Code`] when no category matches.
    pub fn classify(&self, path: &str) -> Category {
        for (idx, set) in self.sets.iter().enumerate() {
            if set.is_match(path) {
                return ORDERED_CATEGORIES[idx];
            }
        }
        Category::Code
    }
}

// ---------------------------------------------------------------------------
// TOML schema (private)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ClassificationsToml {
    #[serde(default)]
    default: CategoryGlobs,
    #[serde(default)]
    ecosystem: std::collections::BTreeMap<String, EcosystemOverlay>,
}

#[derive(Debug, Default, Deserialize)]
struct CategoryGlobs {
    #[serde(default)]
    generated: Vec<String>,
    #[serde(default)]
    migration: Vec<String>,
    #[serde(default)]
    test: Vec<String>,
    #[serde(default)]
    infra: Vec<String>,
    #[serde(default)]
    ci: Vec<String>,
    #[serde(default)]
    config: Vec<String>,
    #[serde(default)]
    asset: Vec<String>,
    #[serde(default)]
    docs: Vec<String>,
    #[serde(default)]
    code: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EcosystemOverlay {
    #[serde(default)]
    marker: Vec<String>,
    #[serde(flatten)]
    globs: CategoryGlobs,
}

fn merge_into(acc: &mut [Vec<String>; 9], src: &CategoryGlobs) {
    // Index order must match ORDERED_CATEGORIES.
    acc[0].extend(src.generated.iter().cloned());
    acc[1].extend(src.migration.iter().cloned());
    acc[2].extend(src.test.iter().cloned());
    acc[3].extend(src.infra.iter().cloned());
    acc[4].extend(src.ci.iter().cloned());
    acc[5].extend(src.config.iter().cloned());
    acc[6].extend(src.asset.iter().cloned());
    acc[7].extend(src.docs.iter().cloned());
    acc[8].extend(src.code.iter().cloned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_for_empty_repo_loads_default_globs() {
        let dir = tempdir().unwrap();
        let c = Classifier::default_for(dir.path()).unwrap();
        assert!(c.ecosystems().is_empty());
        // Default `test` globs match `tests/foo.rs`.
        assert_eq!(c.classify("tests/foo.rs"), Category::Test);
    }

    #[test]
    fn unmatched_path_defaults_to_code() {
        let dir = tempdir().unwrap();
        let c = Classifier::default_for(dir.path()).unwrap();
        // No ecosystem markers, so `.rs` is not in the default `code`
        // set — but unmatched still falls through to Code.
        assert_eq!(c.classify("src/no_match.weird"), Category::Code);
    }

    #[test]
    fn category_order_matches_q14_chain() {
        let expected = [
            Category::Generated,
            Category::Migration,
            Category::Test,
            Category::Infra,
            Category::Ci,
            Category::Config,
            Category::Asset,
            Category::Docs,
            Category::Code,
        ];
        assert_eq!(ORDERED_CATEGORIES, expected);
    }
}

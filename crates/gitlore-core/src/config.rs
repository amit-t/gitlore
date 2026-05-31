//! Configuration schema + filesystem layout.
//!
//! gitlore reads its config from `~/.config/gitlore/config.toml` (XDG on
//! Linux, `~/Library/Application Support/gitlore/config.toml` on macOS via
//! the `directories` crate). On first run there is no config file: every
//! field has a default, so a fresh install boots in a usable mode (spec
//! §4.1 / OQ-T resolution row 2).
//!
//! M1 ships the type and defaults only. Disk load/save lands with M10
//! ("Polish + first release" in spec §20).

use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Application identifiers used by [`directories::ProjectDirs`].
///
/// Kept private so callers go through [`config_dir`] / [`state_dir`] /
/// [`data_dir`] and we can swap the strategy later (e.g. for portable mode)
/// without churning every callsite.
const QUALIFIER: &str = "";
const ORGANIZATION: &str = "";
const APPLICATION: &str = "gitlore";

/// Top-level config, deserialized from `config.toml`.
///
/// All fields default — gitlore must be useful with an empty / missing
/// config file (spec §4.1 / §8).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Search subsystem knobs (hybrid weights, etc.). Populated at M4.
    pub search: SearchConfig,
    /// Risk subsystem knobs (factor weights, recency window). Populated at M8.
    pub risk: RiskConfig,
}

/// Search-time scoring weights.
///
/// Defaults match spec §14 ("Search score (hybrid, when embeddings
/// enabled)"). Concrete usage lands at M4.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    /// Lexical term-match weight.
    pub w_lexical: f32,
    /// Path-match weight.
    pub w_path: f32,
    /// Recency-bias weight.
    pub w_recency: f32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        // Lexical-first defaults — spec §20 M4 ("0.5 lexical + 0.3 path +
        // 0.2 recency"). Hybrid weights swap in when embeddings flip on.
        Self {
            w_lexical: 0.5,
            w_path: 0.3,
            w_recency: 0.2,
        }
    }
}

/// Risk-score knobs.
///
/// Defaults match spec §15. Populated at M8.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RiskConfig {
    /// Recency window (days) used by the risk engine.
    pub window_days: u32,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self { window_days: 90 }
    }
}

/// Resolved on-disk config path: `<config_dir>/config.toml`.
///
/// Returns `None` when no home directory can be determined (extremely rare;
/// containers without `$HOME` are the usual cause). Callers must fall back
/// to defaults in that case rather than panic (spec §8).
pub fn config_file_path() -> Option<PathBuf> {
    project_dirs().map(|p| p.config_dir().join("config.toml"))
}

/// Resolved per-OS config directory (parent of [`config_file_path`]).
pub fn config_dir() -> Option<PathBuf> {
    project_dirs().map(|p| p.config_dir().to_path_buf())
}

/// Resolved per-OS state directory (logs, run state). Falls back to
/// `data_local_dir` on platforms where `state_dir` is not distinct.
pub fn state_dir() -> Option<PathBuf> {
    project_dirs().map(|p| {
        p.state_dir()
            .unwrap_or_else(|| p.data_local_dir())
            .to_path_buf()
    })
}

/// Resolved per-OS data directory (cached models, future per-machine
/// artifacts).
pub fn data_dir() -> Option<PathBuf> {
    project_dirs().map(|p| p.data_dir().to_path_buf())
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

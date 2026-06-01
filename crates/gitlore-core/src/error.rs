//! Workspace-wide error type.
//!
//! Every fallible boundary in `gitlore-core`, `gitlore`, and `gitlore-eval`
//! returns [`Result<T>`] (alias for [`std::result::Result<T, Error>`]) so the
//! TUI / CLI can render one consistent human-readable error surface (spec
//! §8: "No opaque panics for common errors").
//!
//! M1 ships the variant skeleton. Concrete variants get filled in as each
//! milestone lands its surface (M2 git, M3 storage, M4 search, ...).

use std::io;
use std::path::PathBuf;

/// Result alias used across the workspace.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Top-level error surfaced to TUI / CLI callers.
///
/// Variants are intentionally coarse at M1 — they widen as each milestone
/// adds new fallible boundaries. New variants land alongside the code that
/// produces them, not speculatively.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Caller is not inside a Git working tree (spec §8: "Friendly errors
    /// when not in a repo").
    #[error("not inside a git repository: {0}")]
    NotARepository(PathBuf),

    /// Config file failed to parse or validate.
    #[error("config error: {0}")]
    Config(String),

    /// I/O failure (filesystem, pipe, ...).
    #[error(transparent)]
    Io(#[from] io::Error),

    /// TOML deserialization failed.
    #[error(transparent)]
    TomlDe(#[from] toml::de::Error),

    /// TOML serialization failed.
    #[error(transparent)]
    TomlSer(#[from] toml::ser::Error),

    /// JSON (de)serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Catch-all for prototype-stage callsites. Replace with a typed variant
    /// before the touching milestone ships.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Build an [`Error::Other`] from any displayable value.
    pub fn other(msg: impl std::fmt::Display) -> Self {
        Error::Other(msg.to_string())
    }
}

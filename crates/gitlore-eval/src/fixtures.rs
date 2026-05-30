//! Fixture loaders for evaluation scenarios.
//!
//! Public fixtures ship in-tree under `crates/gitlore-eval/fixtures/`.
//! Private fixtures (`eval-private` feature) live outside the repo and are
//! mounted by the self-hosted lane.

use std::path::Path;

/// Errors from fixture loading.
#[derive(Debug)]
pub enum FixtureError {
    /// I/O failure reading the fixture file.
    Io(std::io::Error),
    /// Parse failure (YAML schema mismatch, invalid structure, etc.).
    Parse(String),
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FixtureError::Io(e) => write!(f, "fixture io error: {e}"),
            FixtureError::Parse(msg) => write!(f, "fixture parse error: {msg}"),
        }
    }
}

impl std::error::Error for FixtureError {}

impl From<std::io::Error> for FixtureError {
    fn from(e: std::io::Error) -> Self {
        FixtureError::Io(e)
    }
}

/// Load a YAML fixture file from disk and parse it as `T`.
pub fn load_yaml<T>(path: &Path) -> Result<T, FixtureError>
where
    T: serde::de::DeserializeOwned,
{
    let bytes = std::fs::read(path)?;
    serde_yaml::from_slice(&bytes).map_err(|e| FixtureError::Parse(e.to_string()))
}

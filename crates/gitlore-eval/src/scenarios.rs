//! Scenario registry: maps a name to a runnable evaluation.
//!
//! Public scenarios ship in-tree. Private-fixture scenarios are compiled in
//! only when the `eval-private` feature is enabled, which the self-hosted
//! lane sets per Q6a.

use std::fmt;

/// Errors returned from scenario lookup / execution.
#[derive(Debug)]
pub enum ScenarioError {
    /// No scenario was registered under the given name in this build.
    NotFound(String),
    /// The scenario ran but failed its acceptance threshold or errored out.
    Failed(String),
}

impl fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScenarioError::NotFound(name) => write!(f, "scenario not found: {name}"),
            ScenarioError::Failed(msg) => write!(f, "scenario failed: {msg}"),
        }
    }
}

impl std::error::Error for ScenarioError {}

/// Run the scenario identified by `name`.
///
/// Returns `Ok(())` when the scenario executes and meets its acceptance bar,
/// `Err(ScenarioError::NotFound)` when no scenario matches, and
/// `Err(ScenarioError::Failed)` when the scenario ran but its metric fell
/// below threshold.
pub fn run(name: &str) -> Result<(), ScenarioError> {
    #[cfg(feature = "eval-private")]
    {
        if private::has(name) {
            return private::run(name);
        }
    }
    Err(ScenarioError::NotFound(name.to_string()))
}

/// Names of every scenario compiled into the current binary.
pub fn list() -> Vec<&'static str> {
    #[allow(unused_mut)]
    let mut names: Vec<&'static str> = Vec::new();
    #[cfg(feature = "eval-private")]
    names.extend(private::names());
    names
}

#[cfg(feature = "eval-private")]
mod private {
    //! Private-fixture scenarios. Bodies land alongside fixture mounts on the
    //! self-hosted lane. The stubs here keep the compile clean until then.

    use super::ScenarioError;

    pub fn has(_name: &str) -> bool {
        false
    }

    pub fn run(name: &str) -> Result<(), ScenarioError> {
        Err(ScenarioError::NotFound(name.to_string()))
    }

    pub fn names() -> Vec<&'static str> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_scenario_returns_not_found() {
        match run("nope") {
            Err(ScenarioError::NotFound(name)) => assert_eq!(name, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}

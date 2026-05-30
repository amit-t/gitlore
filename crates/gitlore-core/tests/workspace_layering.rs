//! Enforces strict workspace layering per ADR-005.
//!
//! Tier graph:
//!
//! ```text
//!   gitlore (bin)        gitlore-eval (lib+bin)
//!         \                    /
//!          \                  /
//!           +-> gitlore-core (lib) <-+
//! ```
//!
//! Invariants asserted:
//!
//!   1. `gitlore-core` has zero intra-workspace dependencies.
//!   2. `gitlore`      depends only on `gitlore-core` within the workspace.
//!   3. `gitlore-eval` depends only on `gitlore-core` within the workspace.
//!   4. The intra-workspace dependency graph is acyclic.
//!
//! Only `Normal` and `Build` dependency kinds are considered. Dev-dependencies
//! are exempt: rustc allows dev-dep cycles since they never link into the
//! published artifact, and forbidding them would block the eventual
//! `gitlore-core` <-> `gitlore-eval` test-fixture sharing pattern.
//!
//! Requires `cargo_metadata` as a dev-dependency in
//! `crates/gitlore-core/Cargo.toml`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use cargo_metadata::{DependencyKind, Metadata, MetadataCommand};

const CORE: &str = "gitlore-core";
const GITLORE: &str = "gitlore";
const EVAL: &str = "gitlore-eval";

#[test]
fn workspace_layering_matches_adr_005() {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let metadata = MetadataCommand::new()
        .manifest_path(&manifest_path)
        .no_deps()
        .exec()
        .expect("cargo metadata must succeed");

    let members: BTreeSet<String> = metadata
        .workspace_packages()
        .iter()
        .map(|p| p.name.to_string())
        .collect();

    for required in [CORE, GITLORE, EVAL] {
        assert!(
            members.contains(required),
            "workspace must contain crate `{required}` per ADR-005; members were {members:?}"
        );
    }

    let graph = build_intra_workspace_graph(&metadata, &members);

    // (1) gitlore-core: zero workspace deps.
    let core_deps = graph.get(CORE).expect("gitlore-core present in graph");
    assert!(
        core_deps.is_empty(),
        "{CORE} must have zero intra-workspace dependencies per ADR-005, \
         found: {core_deps:?}"
    );

    // (2)+(3) Upper-tier crates may depend only on gitlore-core.
    let allowed: BTreeSet<String> = [CORE.to_string()].into_iter().collect();
    for upper in [GITLORE, EVAL] {
        let deps = graph
            .get(upper)
            .unwrap_or_else(|| panic!("{upper} present in graph"));
        let illegal: BTreeSet<&String> = deps.difference(&allowed).collect();
        assert!(
            illegal.is_empty(),
            "{upper} may depend only on {CORE} within the workspace per ADR-005; \
             illegal deps: {illegal:?}"
        );
    }

    // (4) No cycles in the intra-workspace dependency graph.
    if let Some(cycle) = find_cycle(&graph) {
        panic!(
            "workspace dependency graph must be acyclic per ADR-005, found cycle: {}",
            cycle.join(" -> ")
        );
    }
}

/// Adjacency list: package name -> set of intra-workspace dependency names.
/// Dev-dependencies are intentionally excluded (see module docs).
fn build_intra_workspace_graph(
    metadata: &Metadata,
    members: &BTreeSet<String>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for pkg in metadata.workspace_packages() {
        let entry = graph.entry(pkg.name.to_string()).or_default();
        for dep in &pkg.dependencies {
            if matches!(dep.kind, DependencyKind::Development) {
                continue;
            }
            if members.contains(&dep.name) {
                entry.insert(dep.name.clone());
            }
        }
    }
    graph
}

/// DFS with white/grey/black coloring. Returns the first cycle found as a
/// sequence of node names (start vertex repeated at the end), or `None` if
/// the graph is acyclic.
fn find_cycle(graph: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Color {
        White,
        Grey,
        Black,
    }

    fn dfs(
        node: &str,
        graph: &BTreeMap<String, BTreeSet<String>>,
        color: &mut HashMap<String, Color>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        color.insert(node.to_string(), Color::Grey);
        path.push(node.to_string());

        if let Some(deps) = graph.get(node) {
            for dep in deps {
                match color.get(dep).copied().unwrap_or(Color::White) {
                    Color::Grey => {
                        let start = path
                            .iter()
                            .position(|n| n == dep)
                            .expect("grey node must be on the active DFS path");
                        let mut cycle: Vec<String> = path[start..].to_vec();
                        cycle.push(dep.clone());
                        return Some(cycle);
                    }
                    Color::Black => {}
                    Color::White => {
                        if let Some(cycle) = dfs(dep, graph, color, path) {
                            return Some(cycle);
                        }
                    }
                }
            }
        }

        path.pop();
        color.insert(node.to_string(), Color::Black);
        None
    }

    let mut color: HashMap<String, Color> =
        graph.keys().map(|k| (k.clone(), Color::White)).collect();
    let mut path: Vec<String> = Vec::new();

    for start in graph.keys() {
        if color.get(start).copied() == Some(Color::White) {
            if let Some(cycle) = dfs(start, graph, &mut color, &mut path) {
                return Some(cycle);
            }
        }
    }
    None
}

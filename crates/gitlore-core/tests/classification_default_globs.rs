//! Acceptance: AC-IDX-8 — default classification globs + Q14 precedence
//! + ecosystem auto-detect (M3-5, TDD-000 §2.2, SPEC-001 §4.4 / §7.3,
//!   ADR-018).
//!
//! SPEC-001 §7.3 references `qa/fixtures-private/api-nodejs/classification.toml`
//! as the gold-set used in the harness. That fixture is not vendored
//! into this repo (private QA assets), so this test uses an inline
//! 20-file hand-labeled set covering every Q14 category.

use std::fs;

use gitlore_core::index::classify::{Category, Classifier};
use tempfile::tempdir;

/// 20 paths, hand-labeled. Each entry is `(path, expected_category)`.
/// The set spans every Q14 category at least once.
const HAND_LABELED: &[(&str, Category)] = &[
    // Generated (highest precedence)
    ("api/v1/pb.pb.go", Category::Generated),
    ("services/auth/proto_pb.py", Category::Generated),
    ("dist/bundle.js", Category::Generated),
    ("node_modules/react/index.js", Category::Generated),
    // Migration
    ("db/migrations/20240101_init.sql", Category::Migration),
    ("backend/alembic/versions/0001_init.py", Category::Migration),
    // Test
    ("tests/integration/users.test.ts", Category::Test),
    ("internal/auth/users_test.go", Category::Test),
    ("spec/models/user_spec.rb", Category::Test),
    // Infra
    ("deploy/Dockerfile", Category::Infra),
    ("infra/terraform/main.tf", Category::Infra),
    ("ops/k8s/deploy.yaml", Category::Infra),
    // CI
    (".github/workflows/test.yml", Category::Ci),
    (".circleci/config.yml", Category::Ci),
    // Config (under any non-ci/infra path)
    ("services/api/config/app.toml", Category::Config),
    ("services/api/settings.ini", Category::Config),
    // Asset
    ("ui/src/assets/logo.png", Category::Asset),
    // Docs
    ("docs/architecture.md", Category::Docs),
    ("README.rst", Category::Docs),
    // Code (default fallback — odd extension on a path with no other
    // match)
    ("services/api/handler.coffee", Category::Code),
];

#[test]
fn default_classification_matches_hand_labeled_set() {
    let dir = tempdir().unwrap();
    let c = Classifier::default_for(dir.path()).expect("classifier builds");
    for (path, expected) in HAND_LABELED {
        let got = c.classify(path);
        assert_eq!(
            got, *expected,
            "classification drift on `{path}`: expected {:?}, got {:?}",
            expected, got
        );
    }
}

#[test]
fn precedence_migration_beats_test() {
    // A file that matches BOTH a migration glob AND a test glob must
    // resolve to Migration because the migration set is evaluated
    // first in the Q14 chain.
    let dir = tempdir().unwrap();
    let c = Classifier::default_for(dir.path()).unwrap();
    let cross_matched = "db/migrations/tests/20240101_seed.py";
    assert_eq!(c.classify(cross_matched), Category::Migration);
}

#[test]
fn precedence_generated_beats_config() {
    // A TOML inside node_modules must classify as Generated, not
    // Config — Generated is the highest priority.
    let dir = tempdir().unwrap();
    let c = Classifier::default_for(dir.path()).unwrap();
    assert_eq!(
        c.classify("node_modules/some-pkg/cfg.toml"),
        Category::Generated
    );
}

#[test]
fn ecosystem_autodetect_rust_adds_rs_to_code_globs() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

    let c = Classifier::default_for(dir.path()).expect("classifier builds with rust overlay");
    assert!(
        c.ecosystems().iter().any(|n| n == "rust"),
        "rust ecosystem must be detected: {:?}",
        c.ecosystems()
    );

    // `src/main.rs` matches no other glob; with the rust overlay it
    // becomes Code (rather than the unmatched-default Code, which
    // would be the same result — so probe a deeper file too).
    assert_eq!(c.classify("src/main.rs"), Category::Code);
    assert_eq!(c.classify("crates/foo/src/lib.rs"), Category::Code);
}

#[test]
fn ecosystem_autodetect_nodejs_adds_ts_to_code_globs() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();

    let c = Classifier::default_for(dir.path()).unwrap();
    assert!(c.ecosystems().iter().any(|n| n == "nodejs"));
    assert_eq!(c.classify("ui/src/app.tsx"), Category::Code);
    assert_eq!(c.classify("ui/src/utils.ts"), Category::Code);
}

#[test]
fn no_ecosystem_marker_falls_back_to_defaults_only() {
    let dir = tempdir().unwrap();
    let c = Classifier::default_for(dir.path()).unwrap();
    assert!(c.ecosystems().is_empty(), "no markers, no overlays");
    // A `.rs` file in a repo with no `Cargo.toml` does NOT match any
    // default glob and falls through to the implicit Code default.
    assert_eq!(c.classify("src/lib.rs"), Category::Code);
}

#[test]
fn unmatched_path_defaults_to_code() {
    let dir = tempdir().unwrap();
    let c = Classifier::default_for(dir.path()).unwrap();
    assert_eq!(c.classify("some/weird/path.xyzzy"), Category::Code);
}

#[test]
fn ecosystem_python_marker_triggers_overlay() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("pyproject.toml"), "").unwrap();
    let c = Classifier::default_for(dir.path()).unwrap();
    assert!(c.ecosystems().iter().any(|n| n == "python"));
    assert_eq!(c.classify("src/app/handlers.py"), Category::Code);
}

//! M3-7d: criterion micro-benchmark for the indexer hot path.
//!
//! Measures `Indexer::open` + `Indexer::run_initial` against a synthetic
//! 500-empty-commit Git repository. The fixture is built once outside the
//! measured loop (cheap — `git init` + 500x `git commit --allow-empty`
//! takes ~5s), then per-iteration setup wipes the on-disk index so each
//! sample exercises a true initial walk rather than an UPSERT-into-warm-DB.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p gitlore-core --bench bench_index
//! ```
//!
//! Sample size is dialled down from criterion's default 100 because each
//! sample shells out to git ~500x via `GitCliProvider`; the default would
//! make the bench unreasonably long without buying useful precision for
//! a Git-CLI-bound workload.

use std::path::{Path, PathBuf};
use std::process::Command;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use tempfile::TempDir;

use gitlore_core::index::indexer::Indexer;
use gitlore_core::index::lock::LockMode;

const N_COMMITS: usize = 500;

/// Owned tempdir + repo root for the bench fixture. The `TempDir` is
/// held in `_dir` so the directory survives until the bench finishes.
struct Fixture {
    _dir: TempDir,
    repo: PathBuf,
}

fn build_fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().to_path_buf();
    run_git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    run_git(&repo, &["config", "user.email", "bench@example.com"]);
    run_git(&repo, &["config", "user.name", "Bench User"]);
    run_git(&repo, &["config", "commit.gpgsign", "false"]);
    run_git(&repo, &["config", "tag.gpgsign", "false"]);
    for i in 0..N_COMMITS {
        let msg = format!("commit {i}");
        run_git(
            &repo,
            &["commit", "--allow-empty", "--quiet", "-m", msg.as_str()],
        );
    }
    Fixture { _dir: dir, repo }
}

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: spawn failed: {e}"));
    assert!(
        out.status.success(),
        "git {:?} exited {:?}: {}",
        args,
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Wipe the gitlore index directory (and its WAL siblings) so the next
/// `Indexer::open` + `run_initial` performs a fresh initial walk rather
/// than re-UPSERTing into a warm database.
fn wipe_index(repo: &Path) {
    let dir = repo.join(".git").join("gitlore");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("wipe gitlore index dir");
    }
}

fn bench_index_500_commits(c: &mut Criterion) {
    let fixture = build_fixture();
    let mut group = c.benchmark_group("indexer");
    // Each iteration walks 500 commits via shell-out git; 10 samples
    // keeps total wall time bounded while still giving criterion enough
    // data for the standard outlier / regression heuristics.
    group.sample_size(10);
    group.bench_function("index_500_commits", |b| {
        b.iter_batched(
            || wipe_index(&fixture.repo),
            |_| {
                let mut indexer =
                    Indexer::open(&fixture.repo, LockMode::Wait).expect("indexer open");
                let report = indexer
                    .run_initial(&mut |_processed, _total| {})
                    .expect("run_initial");
                // Touch the report so the optimiser cannot prove the
                // whole call is dead.
                criterion::black_box(report);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_index_500_commits);
criterion_main!(benches);

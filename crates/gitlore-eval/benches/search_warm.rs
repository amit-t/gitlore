//! Criterion micro-benchmark for warm search latency (M4 / item 67).
//!
//! Measures the wall-clock time of a single `SearchOrchestrator::query` call
//! against a 50-commit synthetic in-memory repo — the same fixture that
//! `scenarios::search::synthetic` uses.
//!
//! ## Running
//!
//! ```text
//! cargo bench --bench search_warm -p gitlore-eval
//! ```
//!
//! Results are written to `target/criterion/search_warm/`.
//!
//! ## What this is NOT
//!
//! This bench does not gate CI. That job belongs to
//! `scenarios::perf::search_warm` which runs against the larger private
//! fixture. This bench is a developer convenience so contributors can spot
//! latency regressions before pushing.

use std::process::Command;
use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use gitlore_core::index::indexer::Indexer;
use gitlore_core::index::lock::LockMode;
use gitlore_core::search::clock::SystemClock;
use gitlore_core::search::conn_pool::SearchConnPool;
use gitlore_core::search::orchestrator::SearchOrchestrator;
use gitlore_core::search::types::{Filters, Query};
use gitlore_core::SearchConfig;
use gitlore_eval::scenarios::search::synthetic::QUERIES_AND_SUBJECTS;

/// Build the same 50-commit synthetic repo that `search.synthetic` uses.
fn make_synthetic_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let repo = tmp.path().to_path_buf();

    fn git(repo: &std::path::Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "bench@example.com"]);
    git(&repo, &["config", "user.name", "Bench"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);

    for (i, (_, subject)) in QUERIES_AND_SUBJECTS.iter().enumerate() {
        let f = format!("file_{i}.txt");
        std::fs::write(repo.join(&f), subject.as_bytes()).unwrap();
        git(&repo, &["add", &f]);
        git(&repo, &["commit", "--quiet", "-m", subject]);
    }
    for i in 10..50_u32 {
        let f = format!("pad_{i}.txt");
        std::fs::write(repo.join(&f), format!("pad {i}").as_bytes()).unwrap();
        git(&repo, &["add", &f]);
        git(
            &repo,
            &["commit", "--quiet", "-m", &format!("chore: pad {i}")],
        );
    }
    (tmp, repo)
}

fn setup_orchestrator(repo: &std::path::Path) -> SearchOrchestrator {
    let mut indexer = Indexer::open(repo, LockMode::Wait).expect("indexer");
    indexer.run_initial(&mut |_, _| {}).expect("index");

    let provider = gitlore_core::git::cli::GitCliProvider::new(repo.to_path_buf());
    let loc =
        gitlore_core::index::storage::resolve_index_path(repo, &provider).expect("index_path");
    let pool = SearchConnPool::open(loc.path()).expect("pool");
    let config = SearchConfig::default();
    let clock = Arc::new(SystemClock);
    SearchOrchestrator::new(pool, config, clock)
}

fn bench_search_warm(c: &mut Criterion) {
    let (_tmp, repo) = make_synthetic_repo();
    let orch = setup_orchestrator(&repo);

    // Warm-up query (not timed).
    let warmup = Query {
        text: "retry on timeout".to_string(),
        filters: Filters::default(),
        limit: 50,
    };
    let _ = orch.query(&warmup).ok();

    let mut group = c.benchmark_group("search_warm");
    for (query_text, _) in QUERIES_AND_SUBJECTS.iter().take(5) {
        let q = Query {
            text: query_text.to_string(),
            filters: Filters::default(),
            limit: 50,
        };
        group.bench_function(*query_text, |b| {
            b.iter(|| {
                let result = orch.query(black_box(&q)).expect("bench query");
                black_box(result)
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_search_warm);
criterion_main!(benches);

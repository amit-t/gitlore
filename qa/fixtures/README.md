# `qa/fixtures/` — gitlore-eval public fixture root

This directory holds the **public** evaluation fixtures consumed by the
`gitlore-eval` crate. It is paired with `qa/fixtures-private/` (sibling
directory, not under this one) for self-hosted-only data.

Both roots are gated by the loader in
[`crates/gitlore-eval/src/fixtures.rs`](../../crates/gitlore-eval/src/fixtures.rs).

## Two roots, one contract

| Path                     | Visibility   | Loaded when                                                            |
| ------------------------ | ------------ | ---------------------------------------------------------------------- |
| `qa/fixtures/`           | Public       | Directory exists. Always loaded on every CI lane (public + private).   |
| `qa/fixtures-private/`   | Self-hosted  | `GITLORE_EVAL_FIXTURES_PRIVATE=1` **and** directory exists on disk.    |

This split (open question **Q6a**) keeps the public lane reproducible
from a clean checkout while letting the self-hosted eval-regression
lane reach for higher-fidelity, hand-labelled data that we either do
not have redistribution rights for or do not want to publish.

A missing public directory is **not** a hard error: scenarios that
cannot find their fixtures emit a `ScenarioReport` with
`passed: false` and a clear `summary`, never a panic. Same goes for a
gated-off private set — it simply reports "skipped".

## Status: empty by design

No public fixture data ships at M2. Per open question **Q6a**, Amit
hand-labels the initial query set at M3+. Until then this directory is
the documented contract; nothing data-bearing is committed under it.

A future PR adds the first real public fixture (likely the
`search.api-nodejs` labelled query set at M4) and replaces this note.

## Active perf gates

Perf scenarios are gated by the **private** fixture root, so the public
hosted lane stays fast and reproducible; the self-hosted lane lights up
the gate when the matching fixture is present.

| Scenario                          | Private fixture                       | Budget        | Source                                                      |
| --------------------------------- | ------------------------------------- | ------------- | ----------------------------------------------------------- |
| `perf.cold_index_api_nodejs`      | `qa/fixtures-private/api-nodejs/`     | p95 < 120 s   | M3-7e — SPEC-001 §7.3 cold-index gate                       |

Cold-index runs work like this:

1. The scenario checks `qa/fixtures-private/api-nodejs/` via
   `std::path::Path::new(...).is_dir()`.
2. **Absent** → emit `passed: true` with summary
   `"skipped: private fixture qa/fixtures-private/api-nodejs/ not present
   (lights up on self-hosted lane)"`. No metrics.
3. **Present** → run three cold iterations. Each iteration copies the
   fixture into a fresh `tempfile::tempdir()`, opens the indexer
   (`LockMode::Wait`) + drives `run_initial`, times the wall-clock,
   drops the indexer + tempdir. The p95 (= max for n=3) must stay below
   the budget. Metrics: `p95_ms`, `samples`, `commits_indexed`.

The remaining four perf entries
(`perf.search_warm`, `perf.story_since`, `perf.risk_since`,
`perf.hotspots_path`) are still M2 stubs and replace their stub row
one-for-one when their owning milestone ships.

## Layout (target — empty today)

Fixtures group by eval pillar so the registry name
(`<pillar>.<fixture>[.<variant>]`, see `gitlore-eval --list`) maps
1:1 to the on-disk path:

```
qa/fixtures/
├── search/
│   └── api-nodejs/          # search.api-nodejs       (M4 / TDD-001)
├── story/
│   └── golden/              # story.golden            (M7 / TDD-002)
├── risk/
│   └── spicy-boring/        # risk.spicy-boring       (M8 / TDD-003)
└── perf/
    ├── cold-index/          # perf.cold_index_*       (M3+ / TDD-004)
    ├── search-warm/         # perf.search_warm        (M4+ / TDD-004)
    ├── story-since/         # perf.story_since        (M7+ / TDD-004)
    ├── risk-since/          # perf.risk_since         (M8+ / TDD-004)
    └── hotspots-path/       # perf.hotspots_path      (M9+ / TDD-004)
```

Each leaf is a self-contained fixture: input data + the labels /
ground truth used by its scenario. Exact schema (manifest format, file
naming) lands with the first concrete scenario; the harness is intentionally
permissive until then so the first author is not boxed in.

## Adding a fixture

1. Pick the pillar directory matching your scenario's first dotted
   segment.
2. Drop your fixture in a subdirectory named after the second dotted
   segment of the scenario name.
3. If anything in the fixture cannot be redistributed under this
   repo's licence (MIT OR Apache-2.0), put it in `qa/fixtures-private/`
   instead — same layout, gated by `GITLORE_EVAL_FIXTURES_PRIVATE=1`.

## See also

* `crates/gitlore-eval/src/fixtures.rs` — loader + env gating contract.
* `crates/gitlore-eval/src/scenarios/builtin.rs` — registered catalog.
* ADR-028 — self-hosted eval-regression lane rationale.

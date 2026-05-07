# gitlore — Unified Spec v1

> A read-only terminal utility for **Git history intelligence**: find the right change, understand the story behind it, see where the risk is, and know who works there.

This document reconciles two prior inputs:

- `gitlore_merged_spec.md` — research-driven, narrow framing as "tig + semantic search"
- `gitlore_prd.md` — broader product framing as a four-pillar history intelligence utility (find / explain / assess / locate)

Where the two diverged, this spec resolves to the PRD's broader scope and pulls in the merged spec's tactical specificity (competitive analysis, milestones, packaging, naming). Every divergence and its resolution is logged in §22.

---

## 1. Document Control

- **Product Name:** gitlore (lowercase, single word; binary, crate, formula, winget all `gitlore`)
- **Version of this doc:** Unified v1, 2026-04-23
- **Owner:** Amit Tiwari
- **Status:** Spec → ready for milestone execution
- **Primary audience:** staff/principal engineers, engineering managers, release managers, on-call engineers, platform engineers in large or fast-moving repos

---

## 2. Product Summary

gitlore is a read-only terminal utility that turns raw Git history into a searchable, grouped, risk-aware view of how a codebase changed over time. It does not try to replace `lazygit`, `tig`, or `gitui` as a Git client. It sits in a different lane: **history intelligence, not history navigation.**

The product answers four questions on demand:

1. **Find** — which commit or change introduced this behavior?
2. **Explain** — what changed since the last release, and what story does it tell?
3. **Assess** — which recent changes look risky, and why?
4. **Locate** — who works in this code and what files churn the most?

Every answer is local-first, terminal-native, and explainable. Semantic search is supported but optional — gitlore is useful on first run before any model is configured.

---

## 3. Problem Statement

Existing terminal Git tools (`lazygit`, `tig`, `gitui`, `serie`) are excellent at the *operational* questions: what's the staged diff, where is HEAD, how do branches relate. They are weak at the *interpretive* questions engineers actually ask during release reviews, incident response, debugging, and onboarding:

- What changed since the last deploy?
- Which commits belong to the same feature?
- Which recent changes are risky?
- Which files keep breaking?
- Who usually works in this area?
- Where should I look first?

Today these are answered with `git log | grep`, `git blame`, PR tools, Slack archaeology, and tribal knowledge. The cost of misunderstanding history is highest in exactly the moments where speed matters most — incidents, releases, onboarding into a complex repo.

---

## 4. Competitive Landscape

| Tool | What it does well | What it doesn't do |
|---|---|---|
| **tig** | Pure ncurses history browser; lexical search; mature | No semantic search, no grouping, no risk signal |
| **lazygit** | Full Git TUI, 76k★, PR icons in v0.61 | History view is flat; freezes on 500k+ commit monorepos |
| **gitui** | Rust, fast on large repos | Less feature-complete than lazygit; no grouping |
| **serie** | Pretty `git log --graph`; explicitly scoped narrow | Not a search/analysis tool by design |
| **git-quick-stats** | Bash one-shot reports (churn, contributors) | Not interactive, no search, no narrative |
| **code-maat** | Adam Tornhill's research CLI for code analytics | JVM, batch-only, academic ergonomics |
| **gitlogue / gource** | Replay/visualization | Novelty/visual, not analytical |
| **GitKraken / GitLens** | GUI/IDE integrations | Out of scope (terminal-first audience) |

**Adjacent attempts at semantic commit search:** `nalcos`, `AbhinavArora95/git-log-search` — both dormant, neither a polished TUI, neither indexes diffs.

**Where gitlore wins:** the *interpretive bundle*. No terminal tool today combines (semantic-or-lexical search) + (story grouping) + (heuristic risk scoring) + (hotspots/ownership) in one local, read-only, daily-driver TUI.

**Where gitlore is weakest:** anyone happy with `git log -S 'foo'` plus mental modeling won't see immediate value. The wedge is the *minutes you save under pressure* (incident, release review, onboarding), not the curiosity case.

---

## 5. Positioning

**One-line:** *gitlore turns your Git history into a story you can search, group, and judge — all in the terminal, with no API keys.*

**What it is:** a read-only, terminal-native history intelligence utility. Complementary to `lazygit`/`tig`. You launch it inside any repo and immediately get **search**, **stories**, **risk**, and **hotspots**.

**What it isn't:** not a Git client, not an LLM-powered code chat, not a SaaS, not a replacement for code review tools, not a graph visualizer.

**Why this is the right wedge (not just "tig + semantics"):**

The narrower "tig + semantic search" framing has two problems. First, semantic embedding quality on terse technical commit messages is uneven — it often loses to lexical+filters until per-hunk indexing lands. Second, "I want to grep my commits but with embeddings" is a tepid value proposition for users who already have `git log -S`. The story/risk/hotspot pillars give gitlore a reason to exist on day one *even with semantic search disabled* — and that's exactly when most users will encounter it.

---

## 6. Goals & Non-Goals

### Primary goals (v0)

- Time to first useful answer **under 30 seconds** for common queries
- Useful inside any Git repo with **zero configuration** on first run
- Useful **without network** and **without semantic models** configured
- **Read-only by contract** — never modify the repo
- **Explainable** — every score (search rank, risk, story membership) has a visible breakdown

### Secondary goals

- Practical performance up to ~100k commits on a modern laptop
- Clean upgrade path to richer semantic retrieval as Phase 2
- macOS + Linux first-class for v0; Windows best-effort

### Non-goals (v0)

- Staging, committing, branching, rebasing, cherry-picking — anything that mutates
- Multi-repo federation
- PR comments / review threads / code hosting workflows
- Full line-history blame timeline
- Required LLM or external API
- Replacement for `lazygit`, `tig`, `gitui`

---

## 7. Target Users

**Primary:** staff/principal engineers, engineering managers with technical depth, release managers, on-call engineers, platform engineers in monorepos.

**Secondary:** senior ICs, engineers onboarding into large codebases, SREs investigating change windows, QA tracing regressions.

**Not the target:** beginners learning Git, GUI-first users, teams who want a full Git client.

---

## 8. Jobs To Be Done

### Job 1 — Find the right change
*"When did we add retry logic to checkout?" / "Where did session token validation move?"* Surfaces relevant commits via lexical search out of the box, semantic ranking when enabled.

### Job 2 — Understand the story
*"What changed since v2.8.0?" / "What happened in payments this week?"* Returns grouped change narratives — clusters of related commits with title, time range, key authors, top paths.

### Job 3 — Assess risk
*"Which of yesterday's merges look risky?" / "What pre-release changes deserve a second look?"* Returns commits/stories ranked by an explainable risk score with visible factors.

### Job 4 — Locate hotspots and ownership
*"Who touches auth most often?" / "What files churn most in checkout?"* Returns hotspot views per path with churn, contributor concentration, recent activity, co-change paths, ownership clues.

---

## 9. Product Principles

- **Read-only by contract.** Trust is part of the product.
- **Terminal-first.** Must feel native to engineering workflows.
- **Useful before clever.** Heuristics and lexical first; semantic and ML later.
- **Transparent scoring.** Show why a score is high, always.
- **Local-first.** No network for core behavior; no data leaves the machine by default.
- **Fast enough to use daily.** Under-100ms warm queries, sub-2-min cold index for 10k commits.

---

## 10. v0 Scope

### In scope (v0 / Phase 1)

- `gitlore` binary, launched inside any Git repo
- Repo detection and worktree-aware Git common-dir resolution
- Local index in `.git/gitlore/` (SQLite)
- Initial + incremental indexing with progress UI
- TUI with four modes: **Search**, **Story**, **Risk**, **Hotspots**
- Lexical commit search with metadata-aware ranking
- Diff viewer with syntax highlighting
- Since-ref summaries (`--since v2.8.0`, `between A B`)
- Story grouping (deterministic heuristics, no embeddings required)
- Heuristic risk scoring with factor breakdown
- Hotspots view per path
- Ownership clues (recency + frequency)
- CLI subcommands for scripted use
- Bundled config at `~/.config/gitlore/config.toml`
- macOS + Linux binaries

### Optional in v0 (Phase 1.5, gated behind `gitlore setup-embeddings`)

- Local embedding model (MiniLM-L6-v2 via ONNX) auto-downloaded
- `sqlite-vec` extension for vector search
- Hybrid ranking with semantic + lexical + path + recency

### Explicitly out of v0

- Per-hunk embeddings (Phase 2)
- PR/issue/deploy metadata overlays (Phase 4)
- Multi-repo search (later)
- Any write operations
- Full blame timeline / "narrative blame" (Phase 4)
- Required LLM dependency

---

## 11. Functional Requirements

### 11.1 Repository detection
- Detect whether CWD is inside a Git repo; resolve common dir for worktrees
- Create `.git/gitlore/` cache directory; never write outside it
- Friendly errors when not in a repo, when permissions block cache creation, when the repo is a shallow clone

### 11.2 Indexing
- Ingest commit metadata: sha, parents, author, committer, dates, subject, body, files changed, insertions/deletions, dirs touched, test/config-file flags, revert detection
- Initial full index, incremental on subsequent runs based on `last_indexed_sha` watermark
- Tolerate interruption; resume cleanly; never leave a half-state DB
- Surface progress (commits processed, ETA) in TUI footer and CLI

### 11.3 Search
- Lexical search out of the box (subject, body, paths, author, sha prefix)
- Optional semantic search when embeddings are enabled
- Hybrid scoring with configurable weights (default in §15)
- Filters: `--path`, `--author`, `--since`, `--until`, `--branch`
- Inline result list with score, sha, date, author, subject
- `Enter` opens the diff inline

### 11.4 Story mode
- Group commits using deterministic heuristics: path overlap, time proximity, author overlap, ref boundaries; semantic similarity when available
- Story = title (auto-generated from common subject tokens + top path), member commits, time range, key authors, top paths, optional tags
- Generate stories for a window: `--since <ref>`, `between <a> <b>`
- Explainable: show **why** these commits were grouped (which signals fired)

### 11.5 Risk mode
- Compute a transparent additive risk score per commit and per story
- Inputs (each contributes a visible sub-score):
  - file count
  - directory spread
  - config / infra file touches (configurable globs)
  - code-to-test change ratio
  - prior churn on touched paths (lookup against the index)
  - revert-like patterns (subject starts with "Revert", or subsequent commit reverts this one)
  - release-window proximity (if a tag is nearby)
- UI shows the final label (low/medium/high) and the factor breakdown
- No ML — keep it heuristic and inspectable

### 11.6 Hotspots
- Hotspot view per repo or per path
- Surface: high-churn files (commit count over window), unique-author count, recent activity recency, revert count, co-change paths (files frequently changed together)
- Window is configurable; default is last 90 days

### 11.7 Ownership clues
- Per path, surface top contributors weighted by frequency × recency
- Present as a *clue*, not authority — header text: "Likely experts (based on history)"
- No automatic CODEOWNERS generation in v0

### 11.8 CLI commands
The tool must support at minimum:
- `gitlore` — launch TUI
- `gitlore index` — force/refresh index
- `gitlore search <query>` — print ranked results to stdout
- `gitlore story --since <ref>` — print stories
- `gitlore risk --since <ref> [--path <p>]` — print risk-ranked items
- `gitlore hotspots <path>` — print hotspot table
- `gitlore explain <sha>` — print commit summary + risk breakdown + story (if any)
- `gitlore between <ref-a> <ref-b>` — print combined story + risk view
- `gitlore setup-embeddings` — download model + enable semantic mode
- `gitlore config <get|set>` — read/write config

All CLI commands must support `--json` for scripted consumption.

### 11.9 TUI
- Keyboard-first; mode switcher on Tab
- Modes: Search, Story, Risk, Hotspots
- Three-pane base layout (top bar, left list, right detail) plus footer with key hints + index state
- Inline help overlay (`?`)

---

## 12. Non-Functional Requirements

### Performance targets
- Cold index of a 10k-commit repo: **under 2 minutes** on modern laptop
- Warm search latency: **under 100ms** end-to-end
- Diff render: non-blocking; lazy-load for large diffs
- Incremental index: **sub-second** when nothing has changed

### Reliability
- Never modify the repo (enforced by code review checklist + integration test that runs in a read-only filesystem)
- Degrade gracefully when `sqlite-vec` is missing → fall back to lexical with a banner
- No opaque panics for common errors (missing repo, permission denied, corrupt index)
- Self-heal: on schema mismatch, prompt for re-index rather than crash

### Security & privacy
- Core workflows fully offline
- No telemetry by default
- Embedding model download is the only outbound call; URL+SHA pinned in code
- Future remote-provider support is opt-in per config

### Usability
- Must provide value on first run inside any repo, no flags
- Risk and story labels must be explainable in-UI
- Single binary, no runtime deps

---

## 13. Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                          TUI Layer                            │
│              (ratatui + crossterm; 4 modes)                   │
└────────────────────────┬─────────────────────────────────────┘
                         │
┌────────────────────────▼─────────────────────────────────────┐
│                    Query Orchestrator                         │
│   route by mode → search / story / risk / hotspots → render  │
└──┬──────────────┬─────────────┬──────────────┬───────────────┘
   │              │             │              │
┌──▼────┐  ┌──────▼──────┐ ┌────▼─────┐ ┌──────▼───────┐
│ Search│  │ Story       │ │ Risk     │ │ Hotspots /   │
│ Engine│  │ Clusterer   │ │ Scorer   │ │ Ownership    │
└──┬────┘  └──────┬──────┘ └────┬─────┘ └──────┬───────┘
   │             │              │              │
   └──────┬──────┴──────────────┴──────────────┘
          │
┌─────────▼───────────────┐  ┌────────────────────┐
│  Index Store (SQLite)   │  │  Embedding Engine  │
│   commits, vectors,     │  │  (optional, ONNX)  │
│   hotspots, state       │  │   MiniLM-L6-v2     │
└─────────┬───────────────┘  └────────────────────┘
          │
┌─────────▼────────────────┐
│   Git Layer (trait)      │
│   ├─ CLI backend (v0)    │
│   └─ git2-rs (Phase 2)   │
└──────────────────────────┘
```

### Stack

| Layer | Choice | Notes |
|---|---|---|
| Language | Rust | Matches modern TUI cohort; single binary |
| TUI | `ratatui` + `crossterm` | Standard |
| Git | **CLI backend first**, abstracted behind a `GitRepo` trait; `git2-rs` later | See §22 for tiebreak |
| Storage | SQLite + `sqlite-vec` (optional) | Embedded, zero-config |
| Embeddings | `ort` + `tokenizers`, MiniLM-L6-v2 23MB | Optional, gated behind setup command |
| Async | `tokio` | Background indexing |
| Config | `serde` + TOML | Convention |
| Logging | `tracing` | Structured, file sink |
| CLI | `clap` v4 | Convention |
| Packaging | `cargo-dist` | Cross-platform binaries |
| Diff syntax highlighting | `syntect` | Phase 1 stretch |

---

## 14. Data Model

```sql
CREATE TABLE commits (
    sha TEXT PRIMARY KEY,
    author_name TEXT,
    author_email TEXT,
    committer_name TEXT,
    committer_email TEXT,
    authored_at INTEGER,        -- unix epoch
    committed_at INTEGER,
    subject TEXT,
    body TEXT,
    parent_shas TEXT,           -- JSON array
    files_changed TEXT,         -- JSON array
    insertions INTEGER,
    deletions INTEGER,
    dirs_touched TEXT,          -- JSON array (top-level dirs)
    test_files_changed INTEGER,
    config_files_changed INTEGER,
    is_revert INTEGER,          -- 0/1
    reverted_by_sha TEXT,       -- nullable
    indexed_at INTEGER
);

CREATE TABLE stories (
    id INTEGER PRIMARY KEY,
    title TEXT,
    date_start INTEGER,
    date_end INTEGER,
    member_count INTEGER,
    top_paths TEXT,             -- JSON array
    authors TEXT,               -- JSON array
    risk_score REAL,
    risk_factors TEXT,          -- JSON object
    generated_at INTEGER
);

CREATE TABLE story_members (
    story_id INTEGER,
    sha TEXT,
    PRIMARY KEY (story_id, sha)
);

CREATE TABLE path_stats (
    path TEXT PRIMARY KEY,
    commit_count INTEGER,
    unique_authors INTEGER,
    revert_count INTEGER,
    last_touched INTEGER,
    cochange_paths TEXT         -- JSON object {path: count}
);

-- Optional, only created when semantic mode is enabled
CREATE VIRTUAL TABLE commit_vectors USING vec0(
    sha TEXT PRIMARY KEY,
    embedding FLOAT[384]
);

CREATE TABLE index_state (
    key TEXT PRIMARY KEY,
    value TEXT
);
-- rows: last_indexed_sha, schema_version, model_name, model_version,
--       embeddings_enabled, last_full_reindex_at
```

---

## 15. Scoring

### Search score (hybrid, when embeddings enabled)

```
score = 0.50 × semantic_similarity
      + 0.20 × lexical_match
      + 0.15 × path_relevance
      + 0.15 × recency_decay
```

Lexical-only mode (default) renormalizes:
```
score = 0.50 × lexical_match
      + 0.30 × path_relevance
      + 0.20 × recency_decay
```

Weights are configurable via `~/.config/gitlore/config.toml`.

### Risk score (heuristic, additive, capped at 1.0)

```
risk = w_files   × normalized(file_count)
     + w_dirs    × normalized(dir_spread)
     + w_infra   × infra_touch_indicator     (0 or 1)
     + w_tests   × inverse(test_change_ratio)
     + w_churn   × normalized(prior_path_churn)
     + w_revert  × revert_proximity
     + w_release × release_window_proximity
```

Default weights: `0.20, 0.15, 0.20, 0.15, 0.15, 0.10, 0.05`. All inputs are visible in the UI breakdown ("Why high-risk: 8 files across 4 dirs, touched config/k8s, 0 test changes").

### Story coherence (used to decide cluster boundaries)
Not user-facing; controls grouping aggressiveness. Tunable but ships with a sensible default. Documented in code, not config.

---

## 16. Story / Risk / Hotspot Engines

### Story clusterer
Deterministic, no ML. For each new commit, attach to an existing open story if:
- ≥1 path overlaps with story's top paths AND
- author overlaps OR commit is within 24h of last story member AND
- not separated by a ref boundary (tag, release branch)

Otherwise start a new story. Optional semantic similarity bumps weak path-overlap matches over the threshold.

Title generation: longest common subject prefix + most-touched top-level dir. Fallback: "{N} commits to {top_path}".

### Risk scorer
Per §15. Inputs computed at index time and cached on the commit row. Re-computed lazily for stories.

### Hotspot computer
Computed at index time per path on a rolling 90-day window. Results cached in `path_stats`. Co-change matrix is sparse — only retain pairs that appear together in ≥3 commits.

---

## 17. UX Requirements

### CLI
Sharp and predictable. Every command has `--json`, `--limit`, and respects `--path` / `--since` / `--until` where applicable.

```bash
gitlore search "retry logic in payments"
gitlore story --since v2.8.0
gitlore risk --since HEAD~20 --path services/session
gitlore hotspots src/auth
gitlore explain abc1234
gitlore between v2.7.0 v2.8.0
```

### TUI

```
┌─ gitlore ──────────────────────────── [Search] [Story] [Risk] [Hotspots]  idx 12,431/12,431 ✓ ─┐
│ > retry logic in payments                                                      lexical | filters: -- │
├──────────────────────────────────────┬─────────────────────────────────────────────────────────────┤
│ ● a3f1c0  2026-04-19  yina   0.84    │  commit a3f1c0  yina@example.com  2026-04-19                │
│   Add exponential backoff to charge…│                                                              │
│   b91d4e  2026-04-15  rohan  0.78    │  Add exponential backoff to charge retry path                │
│   Retry inflight stripe webhooks    │                                                              │
│   ...                                │  diff --git a/services/payments/charge.ts b/...              │
│                                      │  …                                                           │
│                                      │  ── Risk: medium ───────────────                             │
│                                      │  3 files · 1 dir · 0 infra · tests:✓ · churn:hi             │
└──────────────────────────────────────┴─────────────────────────────────────────────────────────────┘
  [Tab] mode  [/] search  [j/k] nav  [Enter] open  [?] help  [q] quit
```

### Interaction
- `j/k` or arrows — navigate
- `/` — focus search input
- `Enter` — open selected item
- `Tab` / `Shift-Tab` — switch mode
- `f` — open filter dialog (path/author/since)
- `?` — help overlay
- `q` — quit

---

## 18. Acceptance Criteria

### MVP acceptance
- Launch TUI inside any Git repo and see indexed history within 30s for ≤10k commit repos
- Lexical search returns relevant results without semantic setup
- `gitlore setup-embeddings` enables hybrid ranking and persists the choice
- `story --since <ref>` produces coherent groupings that pass eyeball review on at least 3 sample repos
- `risk --since <ref>` produces a ranked list with visible factor breakdowns
- `hotspots <path>` produces sensible churn + ownership output
- Tool never modifies the repo (verified by RO-filesystem integration test)
- All common errors return human-readable messages, no Rust backtraces

### Quality acceptance
- Eval set of ≥30 hand-labeled queries per personal repo: relevant commit in top-5 ≥80% of the time (lexical baseline ≥60%, hybrid ≥80%)
- Story groupings on a known release window match a hand-built grouping with ≥70% Jaccard similarity
- Risk scores rank a hand-curated "spicy" set above a hand-curated "boring" set (Mann-Whitney U with p<0.05)
- Tool stays useful with embeddings disabled — all four modes work in lexical/heuristic mode

---

## 19. Roadmap

### Phase 1 — Useful on day one (v0, target: 8–10 weeks solo)
Repo detection · index builder · SQLite storage · TUI shell · lexical search · diff view · since-ref summaries · story clustering (heuristic) · risk engine (heuristic) · hotspots · ownership clues · `setup-embeddings` opt-in semantic layer · macOS+Linux release.

### Phase 2 — Better search & filters
Per-hunk embeddings · BM25 lexical scoring · file/path scoping · time/author/branch pre-filters · diff syntax highlighting via `syntect` · query autocomplete.

### Phase 3 — Better context & export
Branch-aware summaries · compare two release windows · markdown report export · ownership view as a first-class mode · `git2-rs` backend for indexing hot path.

### Phase 4 — Collaboration overlays
PR metadata via `gh`/`glab` adapters · issue key linking · deploy/incident annotations · narrative blame timeline · provider-agnostic.

### Phase 5 — Platform
Multi-repo federation · plugin API · Neovim plugin · optional remote embedding providers (OpenAI, Voyage, Ollama).

### Phase 6 — Intelligence layer (only with traction)
Query rewriting · result summarization · commit theming. LLM-gated, opt-in.

---

## 20. Milestones (Ralph-loop-ready)

Each milestone ends with a runnable artifact and green tests.

### M1 — Walking skeleton
`cargo new`; integrate `clap`, `ratatui`, `crossterm`. Empty 3-pane TUI, `q` quits, arrow keys no-op. CI: `cargo fmt --check`, `clippy -D warnings`, `test` on push.

### M2 — Git layer (CLI-backed)
Define `GitRepo` trait. Implement CLI backend that walks HEAD via `git log --format=...` and parses commit metadata + numstat + name-status. Fixture-repo unit tests. CLI: `gitlore dump --json` for verification.

### M3 — SQLite indexer
Schema migration system. Insert commits incrementally based on `last_indexed_sha`. Resume cleanly on interrupt. `gitlore index` subcommand with progress bar. Test: re-running `index` on unchanged repo is a no-op.

### M4 — Lexical search + ranking baseline
Implement search over subject/body/paths/author/sha. Rank by 0.5 lexical + 0.3 path + 0.2 recency. `gitlore search "query"` prints ranked results. Eval harness scaffolded (no labeled set yet).

### M5 — TUI wired to search
Search panel + commit list + diff pane. Diff via `git show` (until git2-rs). j/k/Enter/q functional. Mode switcher stub.

### M6 — Since-ref summaries + `between`
`gitlore between A B` walks commits in range, aggregates files/insertions/deletions/authors, prints summary table. Same output reachable in TUI footer for current view.

### M7 — Story engine
Deterministic clusterer per §16. `gitlore story --since <ref>` CLI. Story mode in TUI: left pane shows stories, right pane shows member commits + grouping rationale. Test on 3 fixture repos.

### M8 — Risk engine
Risk scorer per §15. `gitlore risk --since <ref>`. Risk mode in TUI. Factor breakdown visible. Hand-curate "spicy" vs "boring" set on one personal repo and verify ranking direction.

### M9 — Hotspots + ownership
`gitlore hotspots <path>`. Hotspots mode in TUI. Ownership clues panel. Co-change computed at index time, cached.

### M10 — Polish + first release
Config file. Non-panic error messages. README + cast/GIF demo. `cargo-dist` pipeline. Homebrew tap. Shell installer script. Tag v0.1.0.

### M11 — Optional semantic layer
`gitlore setup-embeddings` downloads MiniLM (verified by SHA), enables `sqlite-vec`, enables hybrid scoring. Falls back gracefully when extension missing. Re-run eval set: hybrid ≥80% top-5 on labeled queries.

### M12 — Phase 2 starter
Filters (`--path`, `--author`, `--since/--until`). Syntax highlighting in diff pane. BM25 lexical scoring. Query autocomplete from past queries.

---

## 21. Distribution & Packaging

### macOS
- **Day 1:** own Homebrew tap → `brew install yourname/tap/gitlore`
- **Later:** submit to homebrew-core when ≥75★ + ≥30-day-old maintained repo

### Linux
- `cargo install gitlore` from crates.io
- Homebrew-on-Linux via same tap
- Pre-built tarballs via `cargo-dist`
- Distro packages (AUR, Nix, Debian) — community-maintained, not solo work

### Windows (best-effort v0)
- Submit `winget` manifest to `microsoft/winget-pkgs` once stable
- Maintain a Scoop bucket
- Chocolatey only if requested

### `cargo-dist` config
```toml
[workspace.metadata.dist]
installers = ["shell", "powershell", "homebrew", "msi"]
targets = [
  "x86_64-apple-darwin",
  "aarch64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc"
]
```

### MVP launch surface
Ship M10 with **Homebrew tap + crates.io + shell installer script**. Add winget/Scoop in a Phase-2 polish pass.

---

## 22. Reconciliation Log — Divergences & Resolutions

| # | Topic | Merged spec | PRD | Resolution |
|---|---|---|---|---|
| 1 | Scope framing | "tig + semantic search" | History intelligence (find/explain/assess/locate) | **PRD wins.** Per user direction. Semantic search becomes one of four pillars, optional in v0. |
| 2 | Semantic embeddings | Bundled & required | Optional, lexical-first | **PRD wins.** Bundling is a friction tax; lexical-first lets v0 be useful before any setup. Embeddings ship as `gitlore setup-embeddings`. |
| 3 | Git access | `git2-rs` | CLI first, `git2-rs` later | **PRD wins for v0.** CLI is faster to ship, easier to debug against weird real-world repos, matches user mental model. Abstracted behind `GitRepo` trait so `git2-rs` can replace it in Phase 3 for indexing throughput. |
| 4 | Search weights | 0.7 / 0.2 / 0.1 (sem/recency/lex) | 0.5 / 0.2 / 0.15 / 0.15 (sem/lex/path/recency) | **PRD wins.** 4 factors better reflect what makes a result good; path relevance is high-signal in monorepos. Ships configurable. Lexical-only renormalization documented. |
| 5 | Risk model | Not in scope | Heuristic with explainable factors | **PRD wins.** Adopted as core v0 feature. |
| 6 | Story grouping | Phase 4 ("similar commits") only | Core v0 mode | **PRD wins.** Adopted. Heuristic-first per §16 keeps it shippable. |
| 7 | Hotspots/ownership | Not in scope | Core v0 modes | **PRD wins.** Adopted. |
| 8 | Per-hunk embeddings | Phase 2 | Phase 2 | **Agree.** Phase 2. |
| 9 | Binary size | "Under 50MB excl. model" | Not specified | **Merged spec wins.** Plus model is downloaded on opt-in, not bundled — keeps base binary lean. |
| 10 | Naming | gitlore (chosen, with full availability check) | "GitLore or another name" | **Merged spec wins.** `gitlore` is decided. |
| 11 | Distribution | Detailed: Homebrew tap + cargo-dist + shell installer + winget + scoop | "macOS + Linux for v0" only | **Merged spec wins.** Adopted as §21. |
| 12 | Eval methodology | Hand-labeled 30 queries, MRR/nDCG@10 | "Common internal queries...near the top often enough" | **Merged spec wins** for search. Extended in §18 with story Jaccard and risk Mann-Whitney for the broader scope. |
| 13 | Walking-skeleton order | M1–M9, semantic-first | Phase 1 with all four pillars | **Re-sequenced.** New M1–M12 in §20: lexical & TUI shell first → stories → risk → hotspots → release → optional semantic layer. Each milestone still ships a usable artifact. |
| 14 | Read-only contract | Hard rule | Hard rule | **Agree.** Enforced by RO-filesystem integration test. |
| 15 | Telemetry | Open question | Not addressed | **Open question, default no telemetry.** See §24. |

---

## 23. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Story groupings feel arbitrary | Always show *why* commits were grouped (visible signal list). Prefer fewer clear stories over over-clustering. |
| Risk scores feel like "AI confidence theater" | Heuristic only, fully inspectable, every factor visible in UI. Never present a single opaque number. |
| Lexical search isn't differentiated enough | The differentiation is the *bundle* (search + story + risk + hotspots). Don't sell it as "semantic search" until M11. |
| Embedding quality on technical text | Eval `bge-small-en-v1.5` and `nomic-embed-text` against MiniLM on test set before locking choice. Make swappable. |
| `sqlite-vec` maturity | Vector store behind a trait; can swap for `hnswlib-rs` or embedded `qdrant`. |
| Slow indexing on monorepos | Async, resumable, progress visible. Hard cap initial index at configurable N (default 50k). Phase 3 `git2-rs` swap for hot path. |
| Big repos break TUI responsiveness | All long-running work runs on a `tokio` task; TUI never blocks. |
| Trust in new tool | Read-only contract, OSS from day 1, reproducible builds, signed releases. |
| ONNX binary bloat (when enabled) | Evaluate `candle` as alternative (pure Rust, smaller, fewer pre-built models). |
| Solo-builder scope creep | This spec; milestone gates; each milestone ends with a tag and release notes. |

---

## 24. Open Questions

These need decisions before, or during, the first few milestones. Listed in order of urgency.

1. **License.** Recommend MIT OR Apache-2.0 dual (Rust convention). Confirm.
2. **Default embedding model.** MiniLM-L6-v2 (23MB, fast, okay) vs bge-small-en-v1.5 (33MB, notably better on technical text). **Recommendation:** evaluate both on a personal repo before M11; ship the winner as default. Make swappable via config.
3. **Windows support level for v0.** First-class (CI matrix from M1) or best-effort (winget/Scoop after M10)? **Recommendation:** best-effort. Don't let Windows quirks slow Phase 1.
4. **Telemetry.** None vs opt-in crash reporting (Sentry). **Recommendation:** none in v0. Re-evaluate after 100 active users.
5. **GitHub org strategy.** Personal username vs `gitlore-dev` org. **Recommendation:** personal until ≥1 external contributor.
6. **Eval set construction.** Pick one personal repo you know deeply; hand-label 30 queries before M4 lands. Without this, you're flying blind on ranking quality from M4 onward.
7. **Default risk weights.** §15 ships defaults but they're a guess. **Action:** during M8, build a "spicy/boring" labeled set on a personal repo; tune weights to maximize separation.
8. **Story title quality.** Auto-generation may produce ugly titles. **Action:** during M7, check titles by eye on 3 sample repos. If bad, defer better titling to Phase 2 (don't block release).
9. **Markdown export — Phase 1 or Phase 3?** PRD §22 flags this. **Recommendation:** Phase 3. Don't bloat v0.
10. **Domain.** Reserve `gitlore.dev` early (cheap, optional). Not blocking.

---

## 25. Action Items From This Doc

- [ ] Register `gitlore` on crates.io (squat-prevention)
- [ ] Reserve `github.com/<user>/gitlore`
- [ ] Decide license (MIT OR Apache-2.0 recommended)
- [ ] Pick the personal "eval repo" for the labeled query set
- [ ] Build the labeled query set (30 queries) — block M4 completion on this
- [ ] Build the spicy/boring labeled commit set — block M8 completion on this
- [ ] Begin M1

---

*End of unified spec. The merge point in `gitlore_merged_spec.md` (`<!-- EXTERNAL_ANALYSIS_START -->`) can be replaced with a pointer to this doc.*

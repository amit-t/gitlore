# Contributing to gitlore

gitlore is a solo project (Amit, `@amit-t`) shipped under an OSS-from-day-1
license model. The contribution process is intentionally lightweight, but
three rules are non-negotiable and worth reading before you open a PR.

This file is a stub. The unified spec
(`gitlore_unified_spec.md`) is the source of truth for product, scope, and
acceptance criteria; this file covers only the contribution mechanics that
sit on top of GitHub.

---

## 1. License

gitlore is dual-licensed under **MIT OR Apache-2.0**, the Rust ecosystem
convention. By submitting a contribution, you agree that your contribution
is offered under the same dual license, at the recipient's choice. No
separate CLA is required.

- Per-crate license metadata lives in the workspace `Cargo.toml`:
  `license = "MIT OR Apache-2.0"`.
- The full license texts live in `LICENSE-MIT` and `LICENSE-APACHE` at the
  repo root (added at M10 with the first release; until then, the workspace
  manifest is the authoritative statement).
- Do not introduce a dependency whose license is incompatible with both
  MIT and Apache-2.0. `cargo deny` will be wired in at M10; until then
  please check by hand.

Rationale: dual MIT-or-Apache is the de-facto Rust standard, lets
downstream pick the license that fits their distribution, and the Apache
half carries the patent grant. This is the recommendation from spec §24
question 1, confirmed.

---

## 2. Branch protection (the two-tier gate)

`main` is protected. The full policy lives in `docs/branch-protection.md`
and the decision record is **ADR-028**; the short version is:

| Tier              | Trigger                                | Required to merge                                         |
|-------------------|----------------------------------------|-----------------------------------------------------------|
| **mechanical**    | every PR by default                    | green **public CI**                                       |
| **`tier:judgement`** | reviewer applies the label          | green **public CI** + green **private CI** + 1 approving review |

- **Public CI** (`.github/workflows/ci.yml`) is the matrix of
  `fmt-check`, `clippy`, `test`, and `ro-filesystem-integration` on
  macOS + Linux against Rust 1.75. It runs on every PR. It is required.
- **Private CI** is the `eval-regression` job, gated on the
  `tier:judgement` label and a self-hosted runner. It is a stub today
  (M1) and lights up at M4 once the labeled eval set lands.
- **`tier:mechanical`** is the default tier and does not need the
  private lane.
- **`tier:judgement`** is reviewer-applied. Authors propose a tier in the
  PR template; the reviewer confirms or relabels.

Concretely, you should propose `tier:judgement` whenever your PR touches:

- scoring, ranking, or the hybrid blend (§15);
- the story, risk, or hotspot engine (§16);
- the TUI mode contract or keybindings (§17);
- the read-only contract surface (§22 row 14, §13);
- the labeled fixtures (`qa/fixtures/**`, `qa/fixtures-private/**`);
- the steering documents (`steering/**`);
- CODEOWNERS, the CI workflow, or branch-protection itself.

When in doubt, propose `tier:judgement`. Downgrading is cheap; merging a
judgement-grade change through the mechanical gate is not.

The PR template (`.github/pull_request_template.md`) walks you through
the tier proposal and the matching checklist.

---

## 3. Ralph-driven PR convention

A meaningful chunk of gitlore is built by **ralph**, an autonomous coding
loop that picks one task off `.ralph/fix_plan.md` per worktree, implements
it, and opens a PR. This is a deliberate choice (the project is one
person; ralph is the throughput multiplier) and it changes the PR shape in
two ways you should know about:

1. **Atomicity.** Every ralph PR addresses **one** fix_plan task. Bug
   fixes encountered mid-task are split into follow-up tasks rather than
   bundled in. PRs are small and single-concern by construction.
   Hand-authored PRs are expected to follow the same convention.

2. **PR footer.** Ralph PRs carry an auto-generated footer that names:
   - the fix_plan task line that drove the PR,
   - the steering overrides in effect (any `steering.local/**` entries),
   - the worktree branch name.

   The footer is informational and exists so reviewers can spot drift
   between local steering and the project default without grepping. It
   does **not** replace the PR description; ralph still writes a real
   summary in the body.

Hand-authored PRs do **not** need the footer. They do still need:

- Conventional Commit messages (`feat(scope): ...`, `fix(scope): ...`,
  `chore(scope): ...`, etc.). This is what `cargo-dist` / `release-please`
  will consume at M10.
- A clear scope (`crates/<name>` or a top-level area like `ci`, `docs`,
  `tui`, `eval`). The PR title and the first commit should agree.
- No references to absolute paths outside this repo. Ralph runs from
  worktrees under a user-specific path; those paths must never leak into
  commits.

---

## 4. Working locally

Minimum toolchain (matches the CI matrix):

- Rust **1.75** (stable). Use `rustup default 1.75` or
  `rustup override set 1.75` inside the worktree.
- `cargo fmt`, `cargo clippy`, `cargo test` available (the toolchain
  bundles them).

Before pushing:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --test ro_filesystem --locked
```

All four are required by public CI. If any fail locally they will fail
in CI; the public CI is **not** a "fix it for me" service.

---

## 5. Filing issues

Two issue templates ship in `.github/ISSUE_TEMPLATE/`:

- **`bug_report.md`** — defaults to `tier:mechanical`.
- **`feature_request.md`** — defaults to `tier:judgement`.

The default labels reflect the typical tier for each class of work. A
triager (currently `@amit-t`) will relabel as needed before a PR is
opened against the issue.

---

## 6. Reviewers and CODEOWNERS

`@amit-t` is the sole code owner for v0. `.github/CODEOWNERS` pins
eval-grade paths (`qa/fixtures/**`, `qa/fixtures-private/**`,
`steering/**`, and the governance files) so they cannot be silently
rewritten by a ralph PR. CODEOWNERS triggers a review request; branch
protection (via `tier:judgement`) enforces the merge gate.

If you would like to be added as an additional reviewer, open an issue
proposing the path scope and the tier you want to gate. The default
answer is "let's wait until we have a real second contributor", per spec
§24 question 5.

---

## 7. References

- `gitlore_unified_spec.md` — product, scope, acceptance criteria,
  milestones, open questions.
- `docs/branch-protection.md` — the one-time `gh api` calls that apply
  the protection policy on `main`.
- `.github/workflows/ci.yml` — the public CI lane and the
  `eval-regression` private-lane stub.
- `.github/CODEOWNERS` — eval-grade path pinning.
- `.github/pull_request_template.md` — author-facing tier proposal and
  pre-merge checklist.
- `.github/ISSUE_TEMPLATE/` — bug and feature request templates.

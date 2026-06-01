<!--
PR template for gitlore.

Two-tier merge gate per ADR-028 and `docs/branch-protection.md`:

  - `tier:mechanical`  →  green public CI only (default for refactors,
                          dep bumps, doc-only, deterministic fixes).
  - `tier:judgement`   →  green public CI + green private CI
                          + 1 approving review. Reviewer-applied label.
                          Required whenever the PR touches scoring,
                          ranking, story/risk/hotspot engines, eval set,
                          CODEOWNERS-pinned paths, or the RO contract.

You (the author) propose the tier in the checklist below. A reviewer
confirms or relabels before merge. Do not self-merge a `tier:judgement`
PR — branch protection refuses it.
-->

## Summary

<!-- 1-3 lines. What does this PR do and why. Lead with the answer. -->

## Linked work

- Issue / spec section: <!-- e.g. #42, §15 FR-19, ADR-028 -->
- Milestone: <!-- M1..M12 -->
- Depends on: <!-- other PR numbers, or "none" -->

## Change shape

<!-- Tick all that apply. Helps reviewer route attention. -->

- [ ] Code (crate change inside `crates/`)
- [ ] Tests only
- [ ] Docs only
- [ ] CI / workflow change
- [ ] Dependency bump
- [ ] Fixture change under `qa/fixtures/**`
- [ ] Steering / process change under `steering/**`
- [ ] Other:

## Tier label (author proposal, reviewer confirms)

<!--
Tick exactly one. The reviewer will set the actual GitHub label.

The merge gate is:
  - `tier:mechanical` → green public CI only.
  - `tier:judgement`  → green public CI + green private CI + 1 review.

When in doubt, propose `tier:judgement`. It is cheap to downgrade and
expensive to merge a judgement-grade change without the eval gate.
-->

- [ ] **`tier:mechanical`** — deterministic, no behaviour change relative
      to the documented spec. Examples:
  - Refactor with no observable diff in CLI / TUI output.
  - Dependency bump that passes `cargo test --workspace` unchanged.
  - Documentation, comment, or message-text fix.
  - Test added for existing behaviour (no production code change).
  - Lint or format fix.
- [ ] **`tier:judgement`** — touches semantics that the labeled eval set
      or the human reviewer is meant to gate. Required whenever any of:
  - Scoring weights, ranking formula, or hybrid blend (§15).
  - Story / risk / hotspot engine logic (§16).
  - TUI mode contract (§17) or keybinding semantics.
  - Read-only contract surface (§22 row 14, §13).
  - Labeled fixture sets under `qa/fixtures/**` or
    `qa/fixtures-private/**`.
  - Steering docs under `steering/**`.
  - CODEOWNERS, branch protection, or CI gate definitions.
  - New crate dependency or build pipeline change with runtime impact.

## Tier-label checklist

- [ ] I have proposed a tier above and the proposal matches the change
      shape ticked.
- [ ] If `tier:judgement`, I have confirmed (or noted that I cannot run)
      the private `eval-regression` job, and I am ready for a reviewer.
- [ ] If `tier:mechanical`, I have confirmed that no diff in this PR
      changes scoring, ranking, eval set membership, RO surface, or any
      CODEOWNERS-pinned path. If any does, I will relabel before
      requesting review.

## Public CI gate

- [ ] `cargo fmt --all -- --check` passes locally.
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` passes locally.
- [ ] `cargo test --workspace --locked` passes locally.
- [ ] `cargo test --workspace --test ro_filesystem --locked` passes locally
      (the read-only contract integration test).

## Private CI gate (only if `tier:judgement`)

- [ ] I expect `eval-regression` to pass against `origin/main` baseline,
      or I have noted below the expected directional change in the eval
      numbers and why it is acceptable.
- [ ] Expected eval delta (top-5 hit rate, NDCG@10, or M8 spicy/boring
      separation), if any:

      <!-- describe -->

## Read-only contract

- [ ] This PR does not introduce any code path that writes to the target
      repo outside gitlore's own state directory.
- [ ] If it does, an ADR has been added (or referenced) explaining the
      decision, and the `ro-filesystem-integration` test has been updated
      accordingly.

## Ralph hygiene

<!--
gitlore is developed under a ralph-driven loop (see `CONTRIBUTING.md`).
PRs from ralph carry a footer naming the fix_plan task and the steering
overrides in effect. Hand-authored PRs do not need the footer but should
still follow the conventions below.
-->

- [ ] Commit messages follow Conventional Commits
      (`feat(scope): ...`, `fix(scope): ...`, `chore(scope): ...`).
- [ ] No commit body refers to private paths under
      `~/Projects/...` or anything outside this repo.
- [ ] If ralph-authored, the fix_plan task line is referenced in the
      first commit body.

## Notes for the reviewer

<!-- Anything you want me to look at first. -->

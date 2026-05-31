---
name: Bug report
about: A reproducible defect in gitlore (TUI, indexer, search, eval harness, CLI).
title: "bug: <short summary>"
labels: ["bug", "tier:mechanical"]
assignees: []
---

<!--
Triage hints:
- Default tier is `tier:mechanical` — a deterministic defect that can be
  fixed without judgement-grade scrutiny (crash, panic, off-by-one, broken
  arg parsing, regressed unit test, wrong CLI exit code).
- A reviewer will relabel to `tier:judgement` if the fix touches scoring,
  ranking, the story/risk engine, the labeled eval set, or anything else
  that requires the private eval-regression lane to gate merge. See
  `CONTRIBUTING.md` and `docs/branch-protection.md`.
- Please run the latest released gitlore (or `main`) before filing.
-->

## What happened

<!-- Describe the actual behaviour you observed. One paragraph is fine. -->

## What you expected

<!-- Describe the behaviour you expected. -->

## Steps to reproduce

1.
2.
3.

## Repro repo / fixture

<!--
Smallest repro is best.
- If you can share the repo or a stripped-down fixture, link it here.
- If the repo is private, redact path/author/sha fragments and paste the
  relevant `gitlore dump --json` slice.
- If the bug is non-deterministic, attach the seed / config snippet that
  triggered it.
-->

## Environment

- gitlore version: <!-- `gitlore --version` -->
- OS + arch: <!-- e.g. macOS 14.5 arm64, Ubuntu 22.04 x86_64 -->
- Rust toolchain (if built from source): <!-- `rustc --version` -->
- Terminal / shell: <!-- e.g. iTerm2 + zsh, Alacritty + fish -->
- Embedding model in use (if M11+): <!-- MiniLM / bge-small / off -->

## Logs / panic output

```
<!-- Paste the relevant log output, panic trace, or `RUST_BACKTRACE=1`
     stack here. Trim to the relevant frames. -->
```

## Read-only contract

- [ ] To the best of my knowledge, this bug does **not** involve gitlore
      writing to the target repo. (Spec §22 row 14, ADR enforces RO.)
- [ ] If it does, I have included the offending path in the logs above and
      flagged it explicitly. This is a P0 class of bug.

## Anything else

<!-- Related issues, prior PRs, hypotheses, screenshots, asciicasts. -->

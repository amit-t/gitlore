---
name: Feature request
about: Propose new functionality or a behaviour change for gitlore.
title: "feat: <short summary>"
labels: ["enhancement", "tier:judgement"]
assignees: []
---

<!--
Triage hints:
- Default tier is `tier:judgement` for feature requests because almost any
  behaviour change touches scoring, ranking, the story/risk engine, the
  TUI mode contract, or the read-only invariant. A reviewer may relabel to
  `tier:mechanical` if the change is purely additive plumbing with no
  user-visible behaviour shift (e.g. a new `--json` flag wired through
  existing code paths).
- See `CONTRIBUTING.md` for the tier definitions and
  `docs/branch-protection.md` for what each label gates on merge.
- Before filing, check `gitlore_unified_spec.md` §10 (v0 scope) and §6
  (non-goals). Out-of-scope ideas are still welcome but please flag them
  as "post-v0" in the proposal below.
-->

## Problem

<!--
What user need is unmet today? Be concrete. Quote a query you tried, a
review you couldn't run, a commit cluster you couldn't surface, etc.
"It would be nice if" is not a problem statement.
-->

## Proposed solution

<!--
The shape of the change. Subcommand? TUI mode? Config knob? Ranking factor?
Be specific enough that someone could write the eng-spec from this.
-->

## Acceptance criteria

<!--
How will we know this is done? Mirror the style in §18 of the unified spec:
deterministic, observable, testable. Bonus points for naming the fixture
repo it should pass on.
-->

- [ ]
- [ ]
- [ ]

## Scope / non-goals

<!-- What you are explicitly NOT proposing. Helps avoid scope creep. -->

## Spec / milestone alignment

- Relevant spec sections: <!-- e.g. §11 FR-7, §15 scoring, §20 M8 -->
- Target milestone: <!-- M2..M12, or "post-v0" -->
- Does this require new fixtures? <!-- yes / no; if yes, label class -->

## Read-only contract

- [ ] This feature respects the RO contract (Spec §22 row 14): gitlore
      reads the target repo and writes only to its own state directory.
- [ ] If it does require writing outside gitlore's state dir (e.g. a
      `--write-back` flag), I have explicitly flagged it. This is a major
      design decision and requires an ADR.

## Alternatives considered

<!-- Workarounds you tried, related tools, related issues. -->

## Risk / migration

<!--
If shipping this changes default behaviour, ranking, or output schema,
describe the migration story for existing users. If it adds a new
dependency, name it and the size impact.
-->

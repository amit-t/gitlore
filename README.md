# gitlore

> **Status:** placeholder. This README is a pointer file until milestone
> **M10** (polish + first release), at which point it is rewritten to be
> the user-facing landing page with install instructions, a cast/GIF demo,
> and a feature tour. Until then, the unified spec is the source of truth.

A local-first, read-only TUI for spelunking git history: lexical search,
story clustering, risk and hotspot views, optional semantic re-ranking.
Written in Rust, designed to run against any local git checkout without
ever writing to it.

## Where to look right now (pre-M10)

| You want                                | Read this                              |
|-----------------------------------------|----------------------------------------|
| The full product and engineering spec   | `gitlore_unified_spec.md`              |
| The current milestone & open questions  | `gitlore_unified_spec.md` §20, §24     |
| How to contribute / file an issue       | `CONTRIBUTING.md`                      |
| The two-tier merge gate on `main`       | `docs/branch-protection.md`            |
| Public CI lanes and the private stub    | `.github/workflows/ci.yml`             |
| Issue templates                         | `.github/ISSUE_TEMPLATE/`              |
| PR template (incl. tier-label proposal) | `.github/pull_request_template.md`     |
| CODEOWNERS (Q3 fixture protection)      | `.github/CODEOWNERS`                   |
| Name reservations & squat artefacts     | `reservations.md`, `squat-artifacts/`  |

## License

Dual-licensed under **MIT OR Apache-2.0** (Rust convention). See
`CONTRIBUTING.md` §1 for the rationale. Full license texts will be added
at M10 as `LICENSE-MIT` and `LICENSE-APACHE` at the repo root.

## Owner

`@amit-t` is the sole maintainer for v0. See `CONTRIBUTING.md` §6 for the
reviewer policy.

---

*This file is intentionally minimal. The final README is written at M10
(spec §20 M10: "README + cast/GIF demo, `cargo-dist` pipeline, Homebrew
tap, shell installer script, tag v0.1.0").*

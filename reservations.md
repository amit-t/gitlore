# `gitlore` — name reservations

Status as of 2026-05-07.

## Summary

| Status | Registry | Note |
|--------|----------|------|
| ✅ | crates.io | Published `gitlore 0.0.1` (placeholder). Owner: `amit-t` (`tiwari.m.amit@gmail.com`). |
| ✅ | PyPI | Published `gitlore 0.0.1` (placeholder). Maintainer: `amit-t`. |
| ✅ | GitHub repo `amit-t/gitlore` | Public, README pushed. Repo pre-existed (created 2026-04-23); kept its description. |
| ✅ | GitHub repo `amit-t/homebrew-tap` | Public, empty placeholder. Will host formula at v0.1.0. |
| ✅ | GitHub repo `amit-t/scoop-gitlore` | Public, empty placeholder. Will host manifest at v0.1.0. |
| ❌ | npm | Taken by `dlteel` / `nebulord-dev` since at least 2026-04. v1.5.0 deprecated (renamed to `gitrelic`). Owner unreachable; not contesting. |
| ➡ | winget | Cannot be pre-reserved. Submit manifest at M10 release per spec. |
| ➡ | Domain `gitlore.dev` | **Taken** — registered via Porkbun, resolves to `44.227.65.245` (likely parked). Decide later whether to pursue/contact owner. |

## Live URLs

- crates.io: <https://crates.io/crates/gitlore>
- crates.io API (machine-readable): <https://crates.io/api/v1/crates/gitlore>
- PyPI: <https://pypi.org/project/gitlore/>
- PyPI version: <https://pypi.org/project/gitlore/0.0.1/>
- GitHub `gitlore`: <https://github.com/amit-t/gitlore>
- GitHub `homebrew-tap`: <https://github.com/amit-t/homebrew-tap>
- GitHub `scoop-gitlore`: <https://github.com/amit-t/scoop-gitlore>

## Verification (machine-checked)

- crates.io JSON: `{"name":"gitlore","newest_version":"0.0.1","repository":"https://github.com/amit-t/gitlore","homepage":"https://github.com/amit-t/gitlore","versions":1}` — confirmed.
- PyPI JSON: `{"name":"gitlore","version":"0.0.1","files":2}` — confirmed (HTTP 200 on project page).
- GitHub repos: pushed via `gh repo create … --source=. --push`, `main` branch tracking `origin/main` for all three.

## Placeholder content shipped

All placeholders carry the same description:

> gitlore — a terminal utility for Git history intelligence. Reserved; real
> release coming. See github.com/amit-t/gitlore.

- **Crate** (`crate-gitlore/`): `Cargo.toml` (v0.0.1, MIT OR Apache-2.0), `src/lib.rs` (doc comment only), README, `LICENSE-MIT`, `LICENSE-APACHE`.
- **PyPI** (`pypi-gitlore/`): `pyproject.toml` (hatchling backend, v0.0.1, dual MIT/Apache-2.0), `src/gitlore/__init__.py` (docstring + `__version__`), README. Built sdist + wheel, both passed `twine check`.
- **GitHub READMEs**: each repo has a README with status banner and links to the other reservations.

## Outstanding

1. **npm `gitlore`**: skipped per pre-checked plan. Watch for unsquat/abandonment in case owner releases.
2. **winget**: submit manifest at M10 release. No prep needed now.
3. **Domain `gitlore.dev`**: registered by a third party. Decide later whether to use a different TLD (e.g., `gitlore.io`, `gitlore.tools`) or attempt outreach.
4. **PyPI token hygiene**: token used for first publish has `Entire account` scope. Revoke it and create a project-scoped token (scope = `gitlore`) at <https://pypi.org/manage/account/token/>.
5. **crates.io token hygiene**: similarly — token has `publish-new` + `publish-update` on all crates. Optional: scope to `gitlore` only, or rotate.
6. **At v0.1.0 release**:
   - `cargo publish` real release (placeholder rolls forward; semver allows any version > 0.0.1).
   - `twine upload` real release on PyPI.
   - Push real Homebrew formula to `homebrew-tap`.
   - Push real Scoop manifest to `scoop-gitlore`.
   - Submit winget manifest.

## Local artifacts

`./squat-artifacts/`:

- `crate-gitlore/` — published Rust placeholder source
- `pypi-gitlore/` — published PyPI placeholder source (incl. `dist/`)
- `gh-gitlore/` — GitHub repo working copy
- `gh-homebrew-tap/` — tap working copy
- `gh-scoop-gitlore/` — bucket working copy

Inspect or delete after confirming everything is in order.

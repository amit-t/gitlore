# Prompt: reserve the `gitlore` name across registries

Paste the block below into a fresh Claude Code session in an empty working directory. Before you do, make sure the prerequisites listed at the bottom are in place — Claude will pause and ask you if any are missing, but it saves time to front-load them.

---

## The prompt

```
I'm about to build a Rust CLI called `gitlore` — a read-only terminal utility for
Git history intelligence (search, story grouping, risk scoring, hotspots). Before
writing any code, I need to reserve the name `gitlore` across every relevant
package registry so nobody else takes it while I build. Prior availability check
(may be stale, re-verify as you go):

  crates.io ✅ available
  PyPI      ✅ available
  npm       ❌ taken (skip — confirm still taken, then move on)
  GitHub    ✅ (github.com/<me>/gitlore not yet created)
  Homebrew  ✅ (own tap, not homebrew-core)

Your job: reserve the name on every registry below, idempotently. If a step is
already done, skip it. If a step is blocked on a credential I haven't given you,
STOP and ask me — do not invent workarounds or create throwaway accounts. At the
end, produce a one-page status report with a single line per registry:
✅ reserved / ⏸ blocked-on-credential / ❌ name taken / ➡ deferred.

My details:
- Name: Amit Tiwari
- Email: tiwari.m.amit@gmail.com
- GitHub username: ASK ME before using it — I'll tell you
- Description for placeholders: "gitlore — a terminal utility for Git history
  intelligence. Reserved; real release coming. See github.com/<me>/gitlore."

Rules:
- Keep placeholders minimal but honest. No fake version numbers above 0.0.1.
  README says "Reserved. Real release coming." with a link to the GitHub repo.
- Do not publish anything that claims functionality it doesn't have.
- If `cargo publish` or similar would lock me into a name I can't later replace
  with the real crate, warn me before running it. (cargo allows re-publishing
  higher versions, so the placeholder is fine — but confirm.)
- Don't install unnecessary toolchains; use what's already on the system.

Registries to reserve, in priority order:

### 1. crates.io (highest priority — this IS the primary registry)
- Check: `cargo search gitlore` and confirm still available
- Verify credentials: `cat ~/.cargo/credentials.toml` exists; if not, ask me to
  run `cargo login <token>` first
- Action: in a throwaway dir, create a minimal crate:
    Cargo.toml: name="gitlore", version="0.0.1", edition="2021",
                description="...", license="MIT OR Apache-2.0",
                repository="https://github.com/<me>/gitlore",
                authors=["Amit Tiwari <tiwari.m.amit@gmail.com>"]
    src/lib.rs: a single doc comment
    README.md: "gitlore — reserved. Real release coming."
- `cargo publish --dry-run` first, show me the output, then `cargo publish`

### 2. GitHub (two repos)
- Verify: `gh auth status`
- Create `github.com/<me>/gitlore` — public, with a README pointing at the
  unified spec. Include a short "Status: under construction" banner.
- Create `github.com/<me>/homebrew-tap` — public, empty for now, README
  explaining this will host the gitlore formula when v0.1.0 ships.
- Push an initial commit to each. Don't push the unified spec unless I ask.

### 3. PyPI (defensive — prevents namespace confusion)
- Check: https://pypi.org/project/gitlore/ — confirm 404 / "not found"
- Verify credentials: `cat ~/.pypirc` has a valid token; if not, ask me
- Action: create a minimal package
    pyproject.toml with name="gitlore", version="0.0.1",
    description matches the placeholder description above,
    urls.Homepage points at GitHub repo
    src/gitlore/__init__.py: docstring only
    README.md: "Reserved. gitlore is a Rust CLI — this package prevents
      PyPI namespace confusion. See the GitHub repo."
- Build with `python -m build` (install `build` if missing, use `pipx` if
  available to avoid polluting system python)
- Upload with `twine upload dist/*` — show me the dry run first if twine
  supports it

### 4. Scoop bucket (Windows)
- Create `github.com/<me>/scoop-gitlore` (public, empty).
- README: "This repo will host the Scoop manifest for gitlore when v0.1.0
  ships. See github.com/<me>/gitlore."
- No manifest file yet.

### 5. npm — SKIP (name already taken per prior check; re-verify via
  `npm view gitlore` and log the owner in the report). Target is Rust,
  not Node, so this is only a nice-to-have.

### 6. winget — DEFER. winget names are allocated at manifest submission
  time, not pre-reservable. Note this in the report.

### 7. Domain `gitlore.dev` — DEFER. Note availability in the report
  (check via a `whois gitlore.dev` if whois is available) so I can
  decide whether to buy it. Don't buy anything.

Final deliverable:
- A status report printed to stdout in the format:
    [✅ ⏸ ❌ ➡]  registry  —  one-line note
- A `reservations.md` file in the current working directory that captures
  the same report plus any URLs (PyPI project page, crates.io page, GitHub
  repos) for future reference.
- Any loose placeholder code dirs can be left in a `./squat-artifacts/`
  subdirectory so I can inspect them later.
```

---

## Prerequisites checklist

Before pasting the prompt above, confirm:

- [ ] `cargo` installed and `~/.cargo/credentials.toml` has a crates.io API token (`cargo login`)
- [ ] `gh` CLI installed and authenticated (`gh auth status` is clean)
- [ ] `python3` + `pip` available, plus `build` and `twine` (can install on the fly)
- [ ] PyPI account created and `~/.pypirc` has an API token under `[pypi]`
- [ ] You know your GitHub username (Claude will ask)

## What to watch for

- **crates.io is a one-way door for names.** Once you publish v0.0.1, the name `gitlore` is yours forever (you can't delete, only yank — and yanked names stay owned by you). This is exactly what you want. Claude should still show you the `--dry-run` output before the real publish.
- **PyPI placeholder policy.** PyPI does allow placeholders if you genuinely intend to publish. The README text ("Reserved, real release coming") satisfies this.
- **npm is lost.** The prior research said it's taken. The defensive move there is already covered by the crates.io and GitHub reservations; don't fight for it.
- **winget can't be pre-reserved.** That's fine — submit the manifest at M10 release time per the spec.

## After the run

The report should leave you with live pages at roughly:
- `https://crates.io/crates/gitlore`
- `https://pypi.org/project/gitlore/`
- `https://github.com/<you>/gitlore`
- `https://github.com/<you>/homebrew-tap`
- `https://github.com/<you>/scoop-gitlore`

Keep `reservations.md` — you'll reference it again at M10 when you flip each placeholder to the real v0.1.0.

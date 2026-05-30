# Branch protection — `amit-t/gitlore@main`

**Status:** reference doc. The `gh api` calls below are run **once** by Amit (the
sole repo admin) from a local shell. They are **not** executed by CI and
**must not** be wired into any workflow — the protection rules themselves are
what gate CI, so booting them from CI would be circular and would let a
compromised workflow relax the gate it is meant to enforce.

**Source of truth:**

- **ADR-028** — *Branch protection policy for `main`* (decision record).
- **PRD-001 §8.1** — *Release & quality gates* (product-level requirement).

If the policy below ever diverges from ADR-028 or PRD-001 §8.1, the ADR / PRD
win and this doc is wrong; fix this doc, do not silently re-run `gh api` with
different settings.

---

## 1. Policy summary

Two-tier gate on every PR targeting `main`:

| Tier | Trigger | Required to merge |
|---|---|---|
| **Default (all PRs)** | every PR to `main` | green **public CI** |
| **`tier:judgement`** | PR carries the `tier:judgement` label | green **public CI** + green **private CI** + **1 approving review** |

Mechanics on the GitHub side:

- Branch protection lists **two** required status checks on `main`:
  `public-ci` and `judgement-gate`.
- `public-ci` is the normal public workflow. It runs on every PR and must pass.
- `judgement-gate` is a small repo-internal workflow (defined in
  `.github/workflows/judgement-gate.yml`, tracked separately) that:
  - inspects the PR's labels and approving-review count via the GitHub API,
  - if the PR has `tier:judgement`: passes **only** when (a) the private CI
    workflow has reported success for this commit and (b) ≥1 approving review
    is recorded,
  - if the PR does **not** have `tier:judgement`: passes immediately (no-op).

This pattern — "always-required gate workflow that internally checks the
label" — is the only way to express *label-conditional* requirements on the
classic Branch Protection API. We deliberately do **not** use the newer
*Repository Rulesets* API for `main`; ADR-028 records the choice (single
admin, simpler audit, no ruleset/protection-rule precedence surprises).

Branch-level `required_pull_request_reviews` is **deliberately left null**.
Approval enforcement for `tier:judgement` lives inside `judgement-gate` so
that ordinary PRs (docs, chores, dependency bumps) can be self-merged by the
sole maintainer without ceremony, while judgement-tier changes still require
a human reviewer.

---

## 2. Prerequisites

Run from a shell where:

1. `gh auth status` shows you authenticated as `amit-t` (or another account
   with **admin** on `amit-t/gitlore`).
2. The two workflow files are already merged on `main`:
   - `.github/workflows/public-ci.yml` exposing a job named `public-ci`
   - `.github/workflows/judgement-gate.yml` exposing a job named
     `judgement-gate`
   The required-status-check names below **must match the job names** exactly,
   or the protection will block every PR forever waiting for checks that
   never report.
3. The `tier:judgement` label exists on the repo (`gh label create
   tier:judgement -c '#B60205' -d 'Requires private CI + 1 approval'` if not).

---

## 3. Apply protection (one-time)

### 3.1 Main PUT — required checks, linear history, no force-push

```sh
gh api \
  --method PUT \
  -H "Accept: application/vnd.github+json" \
  -H "X-GitHub-Api-Version: 2022-11-28" \
  /repos/amit-t/gitlore/branches/main/protection \
  --input - <<'JSON'
{
  "required_status_checks": {
    "strict": true,
    "checks": [
      { "context": "public-ci" },
      { "context": "judgement-gate" }
    ]
  },
  "enforce_admins": true,
  "required_pull_request_reviews": null,
  "restrictions": null,
  "required_linear_history": true,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "block_creations": false,
  "required_conversation_resolution": true,
  "lock_branch": false,
  "allow_fork_syncing": true
}
JSON
```

Field-by-field rationale:

- `required_status_checks.strict: true` — PRs must be up-to-date with `main`
  before the checks can be considered passing. Prevents "passed yesterday on
  a stale base" merges.
- `required_status_checks.checks` — the `checks` form (object with `context`
  and optional `app_id`) is the documented replacement for the legacy
  `contexts: ["..."]` array. We omit `app_id` so any app may report the check;
  if a third-party app ever starts impersonating these names, pin `app_id`.
- `enforce_admins: true` — admin (Amit) is bound by the same rules. Without
  this, a sole-admin repo has no protection at all in practice.
- `required_pull_request_reviews: null` — see §1; approval enforcement lives
  in `judgement-gate`.
- `restrictions: null` — repo is single-maintainer; no push allow-list needed.
- `required_linear_history: true` — merges to `main` must be squash or
  rebase, never a merge commit. Keeps `git log --first-parent main` readable
  for the history-intelligence use case gitlore itself is built around.
- `allow_force_pushes: false`, `allow_deletions: false` — table stakes.
- `block_creations: false` — does not affect `main` (already exists); set
  false so the doc is copy-pasteable to other branches.
- `required_conversation_resolution: true` — all PR review threads must be
  resolved before merge. Cheap, useful, no downside for a solo project.
- `lock_branch: false` — we still write to `main` via PR merge.
- `allow_fork_syncing: true` — lets forks pull from `main` cleanly.

### 3.2 Require signed commits

```sh
gh api \
  --method POST \
  -H "Accept: application/vnd.github+json" \
  -H "X-GitHub-Api-Version: 2022-11-28" \
  /repos/amit-t/gitlore/branches/main/protection/required_signatures
```

Per ADR-028: every commit on `main` must carry a verified GPG/SSH signature.
This is a separate endpoint from the main PUT — `required_signatures` cannot
be set inside the protection body. The call is idempotent (POST returns 200
if already enabled).

---

## 4. Verify

```sh
gh api /repos/amit-t/gitlore/branches/main/protection \
  | jq '{
      strict: .required_status_checks.strict,
      checks: [.required_status_checks.checks[].context],
      admins: .enforce_admins.enabled,
      reviews: .required_pull_request_reviews,
      linear:  .required_linear_history.enabled,
      force:   .allow_force_pushes.enabled,
      del:     .allow_deletions.enabled,
      convo:   .required_conversation_resolution.enabled
    }'

gh api /repos/amit-t/gitlore/branches/main/protection/required_signatures \
  | jq '.enabled'
```

Expected output:

```json
{
  "strict":  true,
  "checks":  ["public-ci", "judgement-gate"],
  "admins":  true,
  "reviews": null,
  "linear":  true,
  "force":   false,
  "del":     false,
  "convo":   true
}
true
```

If any field disagrees, **re-run §3.1 / §3.2**; do not hand-edit via the
GitHub UI (the UI lossily round-trips some fields, e.g. `checks` ⇄
`contexts`).

---

## 5. Updating the policy

The protection PUT is **idempotent and full-replace**: every call overwrites
the entire protection document. To change one field, re-run the **whole**
§3.1 block with the new value — never PATCH a single field, because GitHub
does not expose a partial-update endpoint for this resource and a hand-rolled
PATCH against `/protection` will silently clear unspecified fields.

When changing required checks:

1. Land the new workflow on `main` first (so the check name exists).
2. Wait for at least one PR to have reported the new check (so GitHub knows
   the context).
3. Then re-run §3.1 with the updated `checks` array.

Reverse order will brick PR merges until the new check reports.

---

## 6. Removing protection (emergency only)

Requires the same admin auth.

```sh
gh api \
  --method DELETE \
  -H "Accept: application/vnd.github+json" \
  -H "X-GitHub-Api-Version: 2022-11-28" \
  /repos/amit-t/gitlore/branches/main/protection
```

After the emergency action, **re-apply §3.1 and §3.2 immediately** and
record an incident note in `docs/incidents/` (the doc itself does not yet
exist — create it on first incident).

---

## 7. Non-goals / explicit non-coverage

- This doc does **not** describe the public-CI or private-CI workflows
  themselves (see `docs/ci-public.md`, `docs/ci-private.md` once they exist).
- This doc does **not** describe how the `tier:judgement` label is applied
  (label policy lives in ADR-028 §3).
- This doc does **not** automate anything in CI. Re-stating §0: branch
  protection is configured **out-of-band** by the human admin, on purpose.

---

## 8. References

- **ADR-028** — Branch protection policy for `main` (decision, rationale,
  alternatives considered including Repository Rulesets).
- **PRD-001 §8.1** — Release & quality gates (product requirement that
  `main` is always release-quality).
- GitHub REST API — *Update branch protection*:
  `PUT /repos/{owner}/{repo}/branches/{branch}/protection`
- GitHub REST API — *Create commit signature protection*:
  `POST /repos/{owner}/{repo}/branches/{branch}/protection/required_signatures`
- GitHub REST API — *Delete branch protection*:
  `DELETE /repos/{owner}/{repo}/branches/{branch}/protection`

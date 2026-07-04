# Add ub-review in 5 minutes

ub-review is a same-model, cache-coherent review team: MiniMax reads the PR
with multiple specialist lanes, selects relevant PR-specific proof, runs it
safely under deterministic guardrails, and posts one review plus a CI gate
result. This guide gets you from zero to reviewed PRs in five minutes.

## Prerequisites

- A Rust project on GitHub with GitHub Actions enabled.
- A MiniMax API key (`MiniMax-M3`).
- The `ub-review` binary (install from source for now; a release artifact is
  tracked in #716).

## Step 1 — Find the ub-review SHA to pin

`enable` pins your workflow to an exact ub-review commit (it never invents a
pin — that's a safety contract). Pick a recent merged SHA from
[EffortlessMetrics/ub-review](https://github.com/EffortlessMetrics/ub-review)
commits, e.g. `cc62168…` (copy the full 40-hex value).

## Step 2 — Run enable

```bash
ub-review enable --mode gate --model minimax --action-sha <40-hex-sha>
```

This writes two files and prints the exact secret to add:

```
ub-review enabled (gate, pinned to cc62168…).

  wrote .github/workflows/ub-review.yml
  wrote .ub-review.toml

Next:
  1. Add MINIMAX_API_KEY as a repository secret:
       repo Settings → Secrets and variables → Actions → New repository secret
  2. Commit the two files and open a pull request.
  3. ub-review will post a MiniMax review and a CI gate result on the PR.
```

Pick your posture:

| `--mode` | What it does | Use when |
|---|---|---|
| `advisory` | Reviews and comments; never blocks. The check is non-required. | First install, calibration |
| `gate` | Reviews + deterministic-floor gate enforcement. Model verdict does not block. **Recommended.** | Normal CI gate |
| `strict` | `gate` + the reporter verdict (changes_requested / uncertain) can block. | Opt-in review-forward after calibration |

See [ADOPTION_MODES.md](ADOPTION_MODES.md) for the full mode table and the
staged path from advisory → gate → strict.

## Step 3 — Add the secret

Add `MINIMAX_API_KEY` as a **repository secret** (Settings → Secrets and
variables → Actions → New repository secret). ub-review reads it from
`${{ secrets.MINIMAX_API_KEY }}` in the workflow — it is never exported to
the step's `env:`, so fork PRs cannot read it.

## Step 4 — Open a PR

Commit `.github/workflows/ub-review.yml` and `.ub-review.toml`, then open a
pull request. ub-review runs on every PR (`opened`, `reopened`,
`ready_for_review`, `synchronize`) and:

1. MiniMax reviews the PR with specialist lanes (tests-oracle, source-route,
   spec-honesty, opposition, …).
2. The proof-planning lane selects relevant PR-specific tests/proof.
3. The deterministic broker validates, leases, runs, and receipts that work.
4. Receipts route back to the lanes; the reporter distills one review.
5. The gate emits pass / fail / inconclusive.

## What ub-review wrote

**`.github/workflows/ub-review.yml`** — the workflow. Pinned to your SHA, with
`review-mode`, MiniMax-on, `posting: review`, and artifact upload. Fork-safe:
`persist-credentials: false`, no `pull_request_target` trigger.

**`.ub-review.toml`** — a minimal config (profile + `[repo]` + `[gate]`). It
is deliberately tiny; the sophistication lives inside the tool. Add proof,
sensors, or lanes later only if the calibration data shows you need them.

## Make it required (gate mode)

In `gate` mode the `ub-review/gate` check can be made a required branch
protection check:

1. Repo Settings → Branches → Branch protection rules → Edit `main`.
2. Add `ub-review/gate` to "Require status checks to pass before merging".
3. The gate enforces the deterministic floor (proof, sensors, policy). MiniMax
   review stays advisory (it posts but does not block) unless you promote to
   `strict` later.

See [ADOPTION_MODES.md](ADOPTION_MODES.md#staged-promotion-checklist) for the
full promotion checklist.

## Troubleshooting

**No review posted / gate is `inconclusive`**
- Check that `MINIMAX_API_KEY` is set and valid. ub-review records provider
  preflight status in the uploaded artifacts under
  `review/provider-preflight-status.json`.
- Fork PRs: the secret is unavailable by default, so ub-review runs in a
  degraded mode. This is expected and safe.

**Gate fails on `cargo-fmt` / `cargo-clippy`**
- These are deterministic checks. Fix the finding and re-push; the gate
  re-evaluates on every synchronize event.

**Source-build is slow**
- Until the release artifact ships (#716), the action builds ub-review from
  source on each run (~2-3 min on `ubuntu-latest`). A release artifact will
  remove this once authorized.

**Bumping the pin**
- To move to a newer ub-review: `ub-review enable --mode gate --action-sha
  <newer-sha> --force` (or edit the `uses:` line directly).

## Related

- [ADOPTION_MODES.md](ADOPTION_MODES.md) — the four modes + staged promotion.
- [ADOPTION_ADVISORY.md](ADOPTION_ADVISORY.md) — the minimal manual setup
  (if you prefer not to use `enable`).
- #721 — the `enable` command issue.
- #720 — the `review-mode` preset vocabulary.

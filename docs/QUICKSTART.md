# Add ub-review in 5 minutes

ub-review is a same-model, cache-coherent review team: MiniMax reads the PR
with multiple specialist lanes, selects relevant PR-specific proof, runs it
safely under deterministic guardrails, and posts one review plus a CI gate
result. This guide gets you from zero to reviewed PRs in five minutes.

## Prerequisites

- A Rust project on GitHub with GitHub Actions enabled.
- A MiniMax API key (`MiniMax-M3`).
- Optional: an OpenCode API key for provider fallback. Without it, MiniMax
  remains operational with reduced provider redundancy.
- The `ub-review` binary. Once a release ships, `enable` resolves it
  automatically; until then, install from source (see [Installing
  ub-review](#installing-ub-review) below).

## Step 1 — Run enable

```bash
ub-review enable --mode gate --model minimax --action-sha <40-hex-sha>
```

`enable` resolves the latest ub-review release and generates a workflow that
**downloads + sha256-verifies the binary** (~seconds per run). The `--action-sha`
is the fallback pin used only when no installable release is resolvable
(pre-release, offline, rate-limited, or incomplete assets) — in that case the
workflow source-builds ub-review instead. `enable` never invents a pin: the SHA
is a safety contract.

Pick a recent merged SHA from
[EffortlessMetrics/ub-review](https://github.com/EffortlessMetrics/ub-review)
commits, e.g. `cc62168…` (copy the full 40-hex value) for the fallback pin.

This writes two files and prints the required and optional secrets:

```
ub-review enabled (gate, release v0.1.0).

  wrote .github/workflows/ub-review.yml
  wrote .ub-review.toml

  The workflow downloads the ub-review v0.1.0 binary and verifies its sha256,
  so each run starts in seconds instead of source-building (~12 min).

Next:
  1. Add MINIMAX_API_KEY as a required repository secret:
       repo Settings → Secrets and variables → Actions → New repository secret
  2. Optionally add OPENCODE for provider fallback. Without it, MiniMax remains
     operational but OpenCode fallback is unavailable.
  3. Commit the two files and open a pull request.
  4. ub-review will post a MiniMax review and a CI gate result on the PR.
```

(When no release is resolvable, the summary instead reports `source-build
pinned to <sha>` and notes the ~12 min cached build — re-run `enable` after a
release ships to switch to the fast binary-download path.)

Pick your posture:

| `--mode` | What it does | Use when |
|---|---|---|
| `advisory` | Reviews and comments; never blocks. The check is non-required. | First install, calibration |
| `gate` | Reviews + deterministic-floor gate enforcement. Model verdict does not block. **Recommended.** | Normal CI gate |
| `strict` | `gate` + the reporter verdict (changes_requested / uncertain) can block. | Opt-in review-forward after calibration |

See [ADOPTION_MODES.md](ADOPTION_MODES.md) for the full mode table and the
staged path from advisory → gate → strict.

## Step 2 — Add provider secrets

Add `MINIMAX_API_KEY` as a **repository secret** (Settings → Secrets and
variables → Actions → New repository secret). ub-review reads it from
`${{ secrets.MINIMAX_API_KEY }}` in the workflow — it is never exported to
the step's `env:`, so fork PRs cannot read it.

Optionally add `OPENCODE` as a repository secret. The generated workflow uses
OpenCode with `mimo-v2.5` for deeper fallback work, while fast fallback lanes
use `deepseek-v4-flash`. If `OPENCODE` is absent, MiniMax continues operating
normally and ub-review records that provider fallback is unavailable.

## Step 3 — Open a PR

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

**`.github/workflows/ub-review.yml`** — the workflow. Pinned to the release
tag (or your SHA, if no release was resolvable), with `review-mode`, MiniMax
primary, optional OpenCode fallback, `posting: review`, and artifact upload. Fork-safe:
`persist-credentials: false`, no `pull_request_target` trigger.

**`.ub-review.toml`** — a minimal config (profile + `[repo]` + `[gate]`). It
is deliberately tiny; the sophistication lives inside the tool. Add proof,
sensors, or lanes later only if the calibration data shows you need them.

## What gets posted to the PR

ub-review does not post on every run. The default config (`post_substantive`)
posts a grouped review **only when at least one finding is substantive**:

- **severity medium or higher** (medium, high, blocker), OR
- **confidence medium-high or higher** (medium-high, high).

Pure lane-status findings (e.g. "this lane produced no output") are excluded —
they are not reviewer-actionable. When no finding is substantive, ub-review
stays quiet (the run is "expected-quiet") and records the result in
`review/calibration.json` without posting boilerplate. This keeps the PR
signal-to-noise ratio high: a posted review means something worth looking at.

To post on every classified run instead, set `summary_only_body = "post_all"`
in `.ub-review.toml` under `[review_body]`. See
[ADOPTION_MODES.md](ADOPTION_MODES.md#posting-posture-summary_only_body) for
the full posting-posture table.

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
- If only OpenCode fallback is unavailable, check the optional `OPENCODE`
  secret. Its absence does not stop MiniMax review.
- Fork PRs: the secret is unavailable by default, so ub-review runs in a
  degraded mode. This is expected and safe.

**Gate fails on `cargo-fmt` / `cargo-clippy`**
- These are deterministic checks. Fix the finding and re-push; the gate
  re-evaluates on every synchronize event.

**Source-build instead of binary download**
- When no installable release was resolvable at `enable` time (unavailable,
  invalid, incomplete, or unreachable), the workflow source-builds ub-review
  (~12 min on the first run, then cached via `Swatinem/rust-cache`). Re-run
  `ub-review enable` after a release ships to regenerate the workflow with the
  fast binary-download path (`install-mode: release`).

**Bumping the pin**
- To move to a newer ub-review: re-run `ub-review enable --mode gate
  --action-sha <40-hex-sha> --force`. If a newer release exists, the regenerated
  workflow pins to it and downloads its binary; otherwise it pins to the SHA and
  source-builds. (You can also edit the `uses:` / `release-version` lines
  directly.)

## Installing ub-review

You need the `ub-review` binary locally only to run `enable` (the generated
workflow installs ub-review itself on the runner). Until a release ships, build
it from source:

```bash
git clone https://github.com/EffortlessMetrics/ub-review
cd ub-review
cargo build --release
# binary: target/release/ub-review
```

Once a release is published, `enable` resolves and the workflow downloads it
automatically; a local install helper may follow.

## Related

- [ADOPTION_MODES.md](ADOPTION_MODES.md) — the four modes + staged promotion.
- [ADOPTION_ADVISORY.md](ADOPTION_ADVISORY.md) — the minimal manual setup
  (if you prefer not to use `enable`).
- #721 — the `enable` command issue.
- #732 — release-aware `enable` (binary download vs source-build).
- #720 — the `review-mode` preset vocabulary.

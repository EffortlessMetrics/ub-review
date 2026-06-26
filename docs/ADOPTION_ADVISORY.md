# Adopting ub-review as a non-blocking advisory reviewer (generic Rust repos)

This is the minimal, repo-agnostic path for adopting `ub-review` as a
**non-blocking advisory reviewer** on a Rust repository that is **not** Bun
(e.g. `perl-lsp-swarm`, `ripr-swarm`, `cargo-allow`). For the Bun preset, see
[ACTION_CONSUMER_BUN.md](ACTION_CONSUMER_BUN.md) instead.

Advisory means: `ub-review` runs the model-cohort review and posts **one grouped
PR review** (neutral `COMMENT` event — never `REQUEST_CHANGES`), and the GitHub
Actions check is **non-required** (`continue-on-error: true`, `fail-on-gate:
'false'`). Findings never block merge; they surface as review comments and
artifacts the human reviewer can read or ignore.

> **The advisory mechanics already exist and work.** This guide only assembles
> the minimal setup. There is no "advisory mode" feature to build — the
> `COMMENT` event (`review_compiler.rs`) + `fail-on-gate: 'false'` gate
> enforcement (`gate.rs`) are the advisory posture.

## What you need

1. One org-level secret: `MINIMAX_API_KEY` (powers the MiniMax model cohort).
2. Two files copied below: `.github/workflows/ub-review.yml` and
   `policy/ub-review.toml`.
3. Same-repo PRs only (org secrets are not safely exposed on fork PRs; this
   workflow deliberately skips forks).

## 1. The workflow

`.github/workflows/ub-review.yml`:

```yaml
name: ub-review

# Advisory AI review: one grouped PR review, never blocks CI.
on:
  pull_request:
    types: [opened, reopened, ready_for_review, synchronize]

permissions:
  contents: read
  pull-requests: write
  checks: write

concurrency:
  group: ub-review-${{ github.event.pull_request.number }}
  cancel-in-progress: false

jobs:
  review:
    # Skip fork PRs: they cannot safely access org secrets.
    if: github.event.pull_request.head.repo.full_name == github.repository
    runs-on: ubuntu-latest
    timeout-minutes: 20
    continue-on-error: true   # advisory: a run failure never reds the PR check
    steps:
      - uses: actions/checkout@v5
        with:
          fetch-depth: 0   # base/head resolution needs history
      - name: Fail clearly if MINIMAX_API_KEY is missing
        env:
          _HAVE_MINIMAX: ${{ secrets.MINIMAX_API_KEY != '' && 'yes' || '' }}
        run: |
          if [ -z "${_HAVE_MINIMAX:-}" ]; then
            echo "::error::MINIMAX_API_KEY is empty or missing; ub-review requires it for model review lanes."
            exit 1
          fi
      - name: Run ub-review
        uses: EffortlessMetrics/ub-review@<PIN>   # see "pinning" below
        with:
          profile: gh-runner
          posting: review          # post one grouped PR review (neutral COMMENT)
          fail-on-gate: 'false'    # advisory: the gate never reds this check
          minimax-api-key: ${{ secrets.MINIMAX_API_KEY }}
          base: origin/${{ github.base_ref }}
          head: HEAD
          out: target/ub-review
          config: policy/ub-review.toml
      - name: Upload review artifacts
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: ub-review-artifacts
          path: target/ub-review
          if-no-files-found: warn
```

## 2. The repo config

`policy/ub-review.toml` (advisory posture, MiniMax-only cohort, core sensors
non-required):

```toml
# Advisory ub-review config. Findings are non-blocking review comments.
review_profile = "gh-runner"
repo.kind = "rust"

[providers]
policy = "minimax-only"   # one model cohort per run (cache-coherent)

[gate]
# Advisory: never turn findings into a red required check. The workflow's
# fail-on-gate: 'false' + continue-on-error already keep the job green;
# this records the posture in the effective-config receipt.
post_review_on = ["opened", "reopened", "ready_for_review", "synchronize"]
```

Sensors default to advisory (`required = false`) unless you opt a tool into
required. To make a tool finding blocking later, add `[tools.<id>.gate]` and
move the workflow to `fail-on-gate: 'true'` + a required check — see
[POLICY_ALLOWLISTS.md](POLICY_ALLOWLISTS.md).

## 3. Pinning

No public release archive exists yet (#343 tracks it). Until then, pin the
action to a **merged main SHA** (every PR on `ub-review` passes its own
self-gate before merge, so merged main SHAs are green-verified). Replace
`<PIN>` with the short SHA of the version you want — for example `54d508a`,
which includes the worker-safety / sensor-semantics / v2-proof fixes
(#675–#683). Bump periodically to pick up fixes; the verifier pins in this
repo catch drift.

```yaml
uses: EffortlessMetrics/ub-review@54d508a   # example; pin to a current merged SHA
```

> **Self-hosted / CX runners:** without a release archive, the action builds
> `ub-review` from source, which needs Rust on the runner. GitHub-hosted
> `ubuntu-latest` runners have Rust; Docker-only CX runners may not. Keep
> advisory jobs on GitHub-hosted runners until #343 ships a release archive.

## What you get

- One grouped PR review per run (neutral `COMMENT` event), with any
  inline comments anchored to the diff.
- A full artifact tree (`target/ub-review/`) uploaded for debugging:
  `review/review.json`, `review/gate_outcome.json`, lane outputs, proof
  receipts, sensor status.
- A single MiniMax model cohort per run (cache-coherent prefix across all
  specialist lanes).
- Never a merge block: the check is non-required and `continue-on-error`.

## Fork-PR safety

This workflow runs `if: head.repo.full_name == github.repository`, so fork PRs
are skipped silently. Forks cannot safely access `MINIMAX_API_KEY`, and
`pull_request_target` on an untrusted checkout is avoided. If you need fork
coverage, use a separate trusted-only dispatch.

## Next steps (when ready)

- **Calibrate:** record a few runs in a `ub-review-calibration.jsonl`
  (true-positive / expected-quiet / false-positive) before raising any
  severity.
- **Add a repo-specific profile:** replace `review_profile = "gh-runner"` with
  a profile calibrated to your repo's real risk surfaces.
- **Promote to blocking:** only after low-noise calibration — flip
  `fail-on-gate` to `'true'`, add `[tools.<id>.gate]` thresholds, and make
  `ub-review/gate` a required branch-protection check.

## Related

- [README.md](../README.md) — project overview and the Bun adoption path.
- [POLICY_ALLOWLISTS.md](POLICY_ALLOWLISTS.md) — tool-gate thresholds and
  required-vs-advisory policy.
- [RUNTIME_PROFILES.md](RUNTIME_PROFILES.md) — runner-size profiles.
- #343 — publish a release archive (removes the source-build pin requirement).
- #678 — the cohort-orchestrator epic (same-model review-team topology).

# Non-blocking advisory evaluation for a generic Rust repository

**First run: model-off, artifact-only, non-required. Keep existing required CI.**
This recipe is an engineering evaluation of the pinned source, not proof of
production support, release-only installation, complete bounded output, or
sole-gate readiness. [Product state](PRODUCT_STATE.md) owns earned capability;
[Quickstart](QUICKSTART.md) covers identity selection and support diagnostics.

## What you need

Use a disposable repository or evaluation branch, GitHub-hosted Linux execution,
a reviewed full Action commit SHA, and the two files below. No provider secret
is needed. Start with same-repository PRs; this recipe deliberately skips forks
rather than claiming fork coverage. The target's existing CI remains unchanged.

Replace `<UB_REVIEW_FULL_COMMIT_SHA>` with the reviewed 40-character commit.
The checkout and uploader pins below are the exact pins already used by this
repository's protected-base independent baseline, not claims that they are the
latest versions. Audit nested Action dependencies as part of tool selection.

## 1. Evaluation workflow

`.github/workflows/ub-review-evaluation.yml`:

```yaml
name: ub-review advisory evaluation

on:
  pull_request:
    types: [opened, reopened, ready_for_review, synchronize]

permissions: {}

concurrency:
  group: ub-review-evaluation-${{ github.event.pull_request.number }}
  cancel-in-progress: true

jobs:
  evidence:
    if: github.event.pull_request.head.repo.full_name == github.repository
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    permissions:
      contents: read
    steps:
      - name: Checkout exact candidate without persisted credentials
        uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09
        with:
          ref: ${{ github.event.pull_request.head.sha }}
          fetch-depth: 0
          persist-credentials: false
      - name: Evaluate without model or publication credentials
        uses: EffortlessMetrics/ub-review@<UB_REVIEW_FULL_COMMIT_SHA>
        with:
          install-mode: source
          review-mode: advisory
          profile: gh-runner
          model-mode: off
          posting: artifact-only
          allow-heavy: 'false'
          config: policy/ub-review.toml
          root: .
          base: origin/${{ github.base_ref }}
          head: ${{ github.event.pull_request.head.sha }}
          pr-head-sha: ${{ github.event.pull_request.head.sha }}
          out: target/ub-review
      - name: Retain the evaluation packet
        if: always()
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a
        with:
          name: ub-review-evaluation-${{ github.event.pull_request.head.sha }}
          path: target/ub-review
          if-no-files-found: error
          retention-days: 7
```

Leave this job out of required-check settings. Non-required does not mean that
an error must be hidden: installation, configuration, or upload failure may
leave a red advisory check and should be investigated. The advisory preset
sets `fail-on-gate: false`; it does not make its evidence complete or correct.

The example source-builds the tool using its selected Action and toolchain.
That is explicit development-source use, not a no-Cargo installation claim.
`allow-heavy: false` leaves heavy witness classes unavailable unless separately
authorized; their absence is not proof that the change is safe. A job timeout
and seven-day retention do not implement process-stream or packet byte limits.
[#1269](https://github.com/EffortlessMetrics/ub-review/issues/1269) owns output
containment. Use disposable runners and inspect size before expanding a pilot.

## 2. Evaluation config

`policy/ub-review.toml`:

```toml
profile = "gh-runner"

[repo]
kind = "rust"
ledger = ""
base = "origin/main"
head = "HEAD"

[review_body]
summary_only_body = "suppress"
```

The workflow's exact base/head arguments select the reviewed revision. Do not
copy Bun-specific Required obligations or claim the generic profile replaces
your repository's CI. Inspect the effective config and tool-selection receipts;
missing tools remain missing evidence. Choose relevant sensors and explicit
proof obligations only after reviewing the target repository's contract.

## Inspect the first packet

Record the exact Action SHA, candidate SHA, admitted revision identity, run and
attempt, artifact digest, selected tools, terminal sensor/proof receipts, and
missing evidence. Inspect `review/gate_outcome.json`, `review/calibration.json`,
and the public payload or skip receipt; compare available task/publication
shadow reports without treating them as production enforcement authority.

This recipe intentionally posts nothing and performs no model investigation.
It therefore cannot prove review usefulness, provider reliability, or confirmed
GitHub delivery. A missing packet is a failed evidence collection, not an
expected-quiet review. Keep diagnostic failure separate from deterministic code
failure, and retain successful Required evidence separately from review prose.

## Move to model-on advisory review deliberately

Model-on review is a separate, explicitly reviewed pilot. Its goal is one useful
grouped neutral `COMMENT` review when material findings warrant it, not a lane
roster or a comment on every run. It requires valid provider output and actual
current-head delivery confirmation. Preparing a payload is not posting it.

Do not simply add provider keys or a write token to the candidate-execution job
above. Separate trusted reviewer/publisher execution from untrusted code and
validate artifact identity, schema, size, and provenance at each handoff. A
same-repository guard, masked logs, or passing a secret via `with:` is not that
isolation boundary. `pull_request_target` is not a shortcut for obtaining keys.
The stable coordinator and external proof remain tracked by
[#658](https://github.com/EffortlessMetrics/ub-review/issues/658) and
[#811](https://github.com/EffortlessMetrics/ub-review/issues/811).

For a reviewed model-on pilot, calibrate accepted/invalid/duplicate/missed
findings, proof-changed conclusions, quiet-clean behavior, delivery failures,
latency, provider cost, and artifact size. Keep existing CI and non-required
review posture while collecting that evidence. Merely seeing no complaints or
one acted-on comment does not authorize required-check promotion.

## Pinning, upgrades, and rollback

Published archive history is in [the release runbook](RELEASE_RUNBOOK.md);
archives do exist, but publication alone does not establish current support.
Keep the Action source SHA distinct from the release version and exact archive
digest. Strict release installation requires independently verified positive
and negative asset/runtime tests, with no silent source fallback.

For an upgrade, retain the previous verified immutable pin and compare the
workflow, config, tool, and artifact identities. Re-run the same evaluation
before expansion. If it regresses, restore that prior workflow/config/pin or
remove the non-required evaluation workflow. Do not change existing required CI,
move historical tags, overwrite release assets, or relabel old receipts.

## Related

- [Quickstart and support evidence](QUICKSTART.md).
- [Mode and promotion boundaries](ADOPTION_MODES.md).
- [Runtime profiles](RUNTIME_PROFILES.md) and [tool policy](POLICY_ALLOWLISTS.md).
- [GitHub secure use](https://docs.github.com/en/actions/reference/security/secure-use).
- [GitHub privileged PR workflow guidance](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target).

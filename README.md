# ub-review

GitHub-runner-first evidence packet and grouped PR review builder for
UB/native-boundary PR review.

This is **not** another generic PR-commenting bot. It builds deterministic
evidence packets, runs bounded BYOK model lanes when configured, validates
inline comments, and submits one grouped Pull Request Review. It is optimized
for cheap CI usage: one GitHub-hosted runner prepares shared context and
advisory receipts once, then cheap model lanes reason over the packet.

First production preset:

```text
bun-ub
```

The initial target is the Bun UB hunt. Other repo presets should be added after
this one proves useful on real PRs.

## Why this exists

Most review bots do this:

```text
PR diff -> one generic LLM -> comments
```

`ub-review` does this:

```text
PR diff
  -> deterministic packet
  -> cheap sensors once
  -> lane-specific evidence packets
  -> MiniMax M3 review lanes by default
  -> validated inline comments
  -> one grouped PR Review
  -> full artifacts
```

LLM tokens are cheap. CI runner time, disk, local analyzer fanout, and reviewer
attention are the constraints. This action keeps CI doing packet work, not long
model orchestration.

## Copy/paste Bun setup

Create `.github/workflows/ub-review-packet.yml` in the Bun fork:

```yaml
name: UB Review Packet

on:
  pull_request:
    types: [opened, ready_for_review]
    paths-ignore:
      - "**/*.md"
      - "docs/**"

permissions:
  contents: read
  pull-requests: write

jobs:
  packet:
    runs-on: ubuntu-latest
    timeout-minutes: 25

    steps:
      - uses: actions/checkout@v5
        with:
          fetch-depth: 0

      - name: Fetch PR base ref
        run: |
          set -euo pipefail
          git fetch --no-tags origin "+refs/heads/${{ github.base_ref }}:refs/remotes/origin/${{ github.base_ref }}"

      - name: Build UB review packet
        id: ub-review
        uses: EffortlessMetrics/ub-review@main
        with:
          preset: bun-ub
          profile: gh-runner
          root: .
          base: origin/${{ github.base_ref }}
          head: HEAD
          out: target/ub-review
          install-tools: 'true'
          tool-bundle: core
          posting: review
          mode: review-direct
          github-token: ${{ github.token }}
          minimax-api-key: ${{ secrets.MINIMAX }}
          minimax-provider-kind: anthropic
          model-mode: auto
          provider-policy: minimax-only
          lane-width: '10'
          model-timeout-sec: '300'
          max-inline-comments: '8'
          model-concurrency: '8'
          max-model-calls: '14'
          fail-on-post-error: 'false'
          allow-heavy: 'false'

      - uses: actions/upload-artifact@v7
        if: always()
        with:
          name: ub-review-packet-${{ github.event.pull_request.number || github.run_id }}
          path: target/ub-review
          if-no-files-found: warn
          retention-days: 7
```

Sensor packet generation does not require secrets. Posting the grouped PR review
uses the scoped `github.token`. The Bun v0 workflow uses direct MiniMax M3 for
all 10 model lanes through `secrets.MINIMAX`. OpenCode Go remains an optional
direct provider for later canary/deep modes, but it is not part of the Bun v0
cutover workflow. `ub-review` does not shell out to OpenCode as an agent
harness. GLM is skipped for v0. Missing model keys are recorded as missing review
evidence instead of treated as a clean run.

Use `EffortlessMetrics/ub-review@main` for the first Bun fork verification.
After that run posts a useful review and uploads a complete packet, tag `v0`
and switch the Bun workflow to `EffortlessMetrics/ub-review@v0`.

After downloading the first Bun artifact, verify the packet contract before
tagging:

```bash
python scripts/verify-bun-review-artifacts.py target/ub-review \
  --min-ok-model-lanes 10 \
  --require-no-model-evidence-failures
```

This check verifies the required packet tree, lane packets, review payload,
post receipt, model receipts, no-LGTM invariant, and basic secret hygiene.

## What it writes

```text
target/ub-review/
  input/
    changed-files.txt
    diff.patch
    diff-context.json

  sensors/
    tokmd/
    ripr/
    unsafe-review/
    ast-grep/
    */ub-review-sensor-status.json

  lanes/
    ub.md
    source-route.md
    tests.md
    arch.md
    opposition.md
    security.md

  candidates/
    candidate-0000-abc123def456.json
    ...

  observations/
    tests-oracle.ndjson
    source-route.ndjson
    ...

  proof_requests/
    proof-001.json
    ...

  questions/
    tests-oracle/
      red-green.json
      ...
    orchestrator-follow-up/
      follow-up-001.json
      ...

  review/
    shared_context.md
    metrics.json
    review.json
    review.md
    candidates.json
    observations.json
    unique_observations.json
    merged_observations.json
    dropped_observations.json
    orchestrator_plan.json
    follow_up_results.json
    follow_up_outputs.json
    follow_up_evidence.json
    proof_requests.json
    proof_request_groups.json
    proof_plan.md
    github-review.json
    github-review-skip.json
    post-result.json
    post-error.json
    github-review-post-payload.json
    post-stdout.json
    post-stderr.txt

  events.ndjson
  candidates.ndjson
  follow_up_questions.ndjson
  follow_up_results.ndjson
  follow_up_outputs.ndjson
  proof_requests.ndjson
  running-summary.md
```

Start with:

```text
target/ub-review/running-summary.md
target/ub-review/lanes/tests.md
target/ub-review/lanes/ub.md
target/ub-review/input/diff.patch
```

## Bun preset

The `bun-ub` preset creates six lane packets:

| Lane | Purpose |
|---|---|
| `ub` | RAB, stale pointer/length, active view vs backing store, worker handoff |
| `source-route` | public API route, sibling paths, PR claim truth |
| `tests` | red/green proof, weak oracles, ASAN/witness posture |
| `arch` | boundary placement, helper shape, smallest complete fix |
| `opposition` | strongest correctness/test/perf/portability objection |
| `security` | UB as exploit primitive, memory corruption, leak/DoS/security framing |

Lane identity and model identity are separate. Static packet prefixes use lane
names only; direct review mode records the provider/model separately in
`review.json` and `review.md`. The Bun v0 direct model pass uses 10 lanes through
direct MiniMax M3 with `provider-policy: minimax-only`. OpenCode Go canary/deep
lanes remain available later through `provider-policy: minimax-primary`,
`opencode-go-canary`, or `opencode-go-wide` once the provider key is proven.

## Sensors

Default core sensors are best-effort:

- `tokmd` for deterministic repository/diff packets and LLM-ready context;
- `ripr` for Rust changed-behavior test-oracle weakness;
- `unsafe-review` for Rust unsafe/native-boundary reviewability;
- `ast-grep` for cheap structural route scans.

Missing sensors are recorded as missing evidence. Missing evidence is never
reported as clean evidence.

Heavy witnesses such as builds, tests, Miri, ASAN, and mutation testing are off
by default. Enable them only behind explicit workflow policy.

## Review posting

`ub-review run` prepares evidence and review artifacts. `ub-review post` submits
`review/github-review.json` as one GitHub Pull Request Review:

```bash
ub-review run --posting review --out target/ub-review
ub-review post --review-json target/ub-review/review/github-review.json
```

Inline comments are only emitted when they pass the diff-line guardrails:
repo-relative path, valid `RIGHT` side line from the PR diff, actionable
severity, high or medium-high confidence, concise body, lane prefix, and
evidence or a disproof condition. Other candidates stay in `review.md` under
summary-only findings.

## Efficient CI stance

The intended cheap path is:

```text
1 runner job
  checkout
  build packet
  run cheap sensors once
  upload artifact
```

Do not run many independent review jobs that rediscover the repository. This
action builds shared context once, runs bounded model lanes over that context,
validates inline candidates, and submits one grouped PR review when configured.

## Inputs

| Input | Default | Meaning |
|---|---|---|
| `preset` | `bun-ub` | Repo preset. |
| `profile` | `gh-runner` | Box profile. |
| `base` | `origin/main` | Base ref. |
| `head` | `HEAD` | Head ref. |
| `out` | `target/ub-review` | Packet output directory. |
| `tool-bundle` | `core` | `none`, `core`, `bun-fast`, or `full`. |
| `install-tools` | `true` | Best-effort sensor install. |
| `setup-rust` | `true` | Select Rust 1.95 with rustup when available. |
| `install-mode` | `auto` | `auto`, `source`, or `path`. |
| `binary-path` | empty | Existing binary path for `install-mode=path`. |
| `allow-heavy` | `false` | Permit heavy witness classes. |
| `posting` | `review` | `review` posts one Pull Request Review; `artifact-only` only writes files. |
| `mode` | `review-direct` | Direct BYOK MiniMax fanout; agent modes are reserved for later. |
| `github-token` | empty | Scoped token for `posting=review`. |
| `minimax-api-key` | empty | MiniMax M3 lane key. |
| `minimax-api-url` | empty | Optional MiniMax API URL override. |
| `minimax-provider-kind` | `anthropic` | MiniMax envelope, `anthropic` or `openai`. |
| `minimax-model` | `MiniMax-M3` | MiniMax model name. |
| `opencode-api-key` | empty | OpenCode Go key for optional direct provider lanes. |
| `opencode-api-url` | empty | Optional OpenCode Go API URL override. |
| `opencode-model` | `minimax-m3` | OpenCode Go canary model. |
| `opencode-endpoint-kind` | `auto` | `auto`, `openai-chat`, or `anthropic-messages`. |
| `model-mode` | `auto` | `auto` or `off`. |
| `provider-policy` | `minimax-primary` | `minimax-primary`, `minimax-only`, `opencode-go-canary`, `opencode-go-wide`, or `auto`. |
| `lane-width` | `10` | Bun model lane width: `6`, `10`, or `20`. |
| `model-timeout-sec` | `300` | Per-model-call timeout. |
| `max-inline-comments` | `8` | Upper bound for validated inline comments. |
| `model-concurrency` | `8` | Planned model lane concurrency. |
| `max-model-calls` | `14` | Upper bound for model review calls. |
| `review-body-max-bytes` | `60000` | Maximum grouped review body size. |
| `ledger-path` | empty | Optional read-only UB ledger path. |
| `ledger-max-bytes` | `65536` | Maximum ledger context bytes. |
| `fail-on-post-error` | `false` | Fail the action when PR review posting fails. |
| `github-summary` | `true` | Append running summary to job summary. |

## Outputs

| Output | Meaning |
|---|---|
| `out` | Output directory containing the full packet. |
| `summary-path` | `running-summary.md`. |
| `events-path` | Append-only `events.ndjson`. |
| `review-json-path` | Internal `review/review.json`. |
| `metrics-json-path` | Review metrics artifact. |
| `github-review-path` | Prepared grouped review payload. |
| `post-result-path` | Successful grouped review post receipt. |
| `post-error-path` | Grouped review post error receipt. |
| `post-payload-path` | Exact grouped review payload submitted to GitHub. |
| `post-stdout-path` | GitHub post response body artifact. |
| `post-stderr-path` | GitHub post stderr artifact. |

## Bootstrap note

Until this repo publishes release binaries, the action builds `ub-review` from
source on the runner. That is slower than the final release-binary path, but it
keeps first adoption token-free and mechanically simple. The consuming workflow
can cache Cargo registry and target directories if needed.

## Codex lane notes

Codex work should follow [docs/CODEX_FINISH.md](docs/CODEX_FINISH.md): one
small green PR at a time, MiniMax M3 primary for v0, GLM skipped until
approved, agent harnesses out of the hot path, and real sensor defects filed in
the matching `*-swarm` repo instead of silently absorbed into `ub-review`.

## Roadmap and calibration

Track the next steps in [docs/ROADMAP.md](docs/ROADMAP.md). The roadmap records
the v0 Bun smoke proof, cleanup work, PR body cleanup, profile extraction path,
and the planned resource-aware orchestrator with proof and resource brokers.

Use [docs/calibration/bun-ub-review-ledger.md](docs/calibration/bun-ub-review-ledger.md)
to record acted-on findings, false premises, parked follow-ups, and review
compiler tuning notes from real Bun fork runs.

## Local development

```bash
cargo generate-lockfile
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

## Rust style

- Rust 2024
- Rust 1.95 MSRV
- `unsafe_code = forbid`
- efficient CI gates
- advisory by default
- one grouped PR Review when posting is configured
- no issue-comment spam or standalone lane posts

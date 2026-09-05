# ub-review

Revision-bound evidence control plane for PR review and targeted CI.

`ub-review` coordinates one evidence transaction over an exact admitted code
state. It decides what deterministic evidence and bounded model investigation a
change needs, brokers only approved proof tasks, reconciles the resulting
claims and receipts, prepares one concise Pull Request Review, and records a
separate CI evidence result.

It is not another generic PR-commenting bot, a fixed wall of AI reviewers, or a
large CI matrix summarized by a model.

```text
exact admitted revision
  -> repository facts + diff/impact context
  -> deterministic sensors and Required proof
  -> relevant model investigation
  <-> approved focused proof and revision-bound receipts
  -> claim reconciliation and final review judgment
  -> one grouped Pull Request Review
  + one current pass / finding / not_proven CI evidence report
  -> retained audit artifacts
```

Today `gate-result` reports `pass | finding | not_proven`, while the legacy
`gate-conclusion` remains enforcement authority. `PASS / FAIL / NOT_PROVEN`
names the eventual `FinalizedOutcome.ci_evidence_result`; a current `finding`
is not automatically target `fail`. See the canonical
[CI outcome vocabulary](docs/PRODUCT_STATE.md#ci-outcome-vocabulary).

## What sets it apart

- **Models investigate; receipts decide.** Models can form hypotheses, propose
  findings, and request semantic proof. Rust resolves any request to an
  approved target and command template. A model verdict is never proof.
- **The reviewer has a controlled lab.** Focused proof can run while
  investigation continues, and its receipt can change only the exact claim it
  tested.
- **Review and CI share evidence, not authority.** Review quality, publication
  success, instrument coverage, and gate sufficiency remain separate results.
- **One required check is the eventual authority surface, not one giant job.**
  The destination is one stable coordinator over independently receipted tasks,
  caches, deadlines, and side effects.
- **Private machinery, public judgment.** Planner state, lane traces, provider
  status, command output, and cost stay in artifacts. The PR surface spends
  attention on material findings, verification questions, and the bounded
  decision.
- **Repository-native programmes replace generic reviewer panels.** Explicit
  architecture, ownership, support, mirror, CI, and proof contracts should
  determine which review programmes apply.

The initial work grew out of UB/native-boundary review, but the control-plane
contract is broader: architecture-aware review and deterministic CI evidence
for material changes.

## Current maturity

Candidate-head delivery transactions, current-head reconciliation, substantial
review/proof substrate, immutable current-run revision joins, and the
pure/replayable TaskLedger exist. Synthetic merge-result delivery binding and
post-confirmed publication finalization remain open. Fast and late sensor
execution is now shadow-observed in the ledger; proof/worker execution and
cross-projection reconciliation remain open through #1266/#956 and #957.
The repository does **not** yet have one live task/resource scheduler,
Required-first shared run plan, authoritative final lead, finalized outcome
enforcement, trusted learning, hostile-head-safe stable coordinator, or
externally calibrated sole-gate operation.

The released action should therefore be evaluated first in advisory or
artifact-only operation. Promotion to a sole required check is a later,
explicit repository-owner decision after exact-head stable-coordinator proof,
rollback/break-glass coverage, and external calibration.

See:

- [Product state](docs/PRODUCT_STATE.md) for the earned capability matrix and
  active implementation front;
- [Architecture](docs/ARCHITECTURE.md) for current versus destination
  authority;
- [advisory adoption](docs/ADOPTION_ADVISORY.md) for a non-blocking start;
- [issue #840](https://github.com/EffortlessMetrics/ub-review/issues/840) for
  the useful-reviewer and trustworthy-gate contract;
- [issue #945](https://github.com/EffortlessMetrics/ub-review/issues/945) for
  PR-sized execution order.

## Inspect before adopting

The setup commands inspect and generate proposals; they do not silently mutate
branch protection:

```bash
ub-review init --profile gh-runner
ub-review enable --inspect --mode gate --model minimax
ub-review audit-ci
ub-review setup-ci --print-pr
```

`init` writes starter configuration and repository guidance. `enable --inspect`
proposes a workflow/configuration for the detected repository. `audit-ci` is
read-only. `setup-ci --print-pr` renders the four new files in the migration
candidate without opening or applying it; `setup-ci --open-pr` opens that
new-files-only PR but never mutates branch protection. Review the generated
Required proof mapping and begin non-blocking; do not treat generated
configuration as sole-gate proof.

What the system never claims: code correctness, UB-freedom, replacement of
security tooling, model findings as proof, prepared output as delivered output,
or missing evidence as clean evidence.

## What it writes

```text
target/ub-review/
  input/
    changed-files.txt
    diff.patch
    diff-context.json

  sensors/
    tokmd/
    cargo-allow/
    ripr/
    unsafe-review/
    ast-grep/
    actionlint/
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
    final_orchestrator_plan.json
    model_stages.json
    follow_up_results.json
    follow_up_outputs.json
    follow_up_evidence.json
    resolved_candidates.json
    final_compiler_input.json
    witnesses.json
    witness_registry.json
    proof_requests.json
    proof_request_groups.json
    proof_receipts.json
    proof_plan.md
    receipt_routes.json
    resource_leases.json
    resource_plan.md
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
  resolved_candidates.ndjson
  model_stages.ndjson
  witnesses.ndjson
  proof_requests.ndjson
  proof_receipts.ndjson
  receipt_routes.ndjson
  resource_leases.ndjson
  running-summary.md
```

Start with:

```text
target/ub-review/running-summary.md
target/ub-review/lanes/tests.md
target/ub-review/lanes/ub.md
target/ub-review/input/diff.patch
```

`resolved_candidates` reconciles `review/candidates.json` with
`review/follow_up_results.json` and `review/follow_up_outputs.json`. It records
unchanged, unresolved, unavailable, resolved, or conflicting candidate state
after follow-up evidence; it is an audit receipt, not reviewer-facing text.

## Bun preset

The `bun-ub` preset loads `profiles/bun-ub-v0.toml` as the Bun review profile.
The runtime profile (`gh-runner`, `cx23`, `cx33`, or `cx43`) supplies box
budgets separately from `runtime/*.toml`.

The profile creates six lane packets:

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
- `cargo-allow` for source-tree exception ledger drift;
- `ripr` for Rust changed-behavior test-oracle weakness;
- `unsafe-review` for Rust unsafe-contract reviewability;
- `ast-grep` for cheap structural route scans;
- `actionlint` for workflow changes.

Missing sensors are recorded as missing evidence. Missing evidence is never
reported as clean evidence.

Heavy witnesses such as builds, tests, Miri, ASAN, and mutation testing are off
by default. Enable them only behind explicit workflow policy.

Custom configs can mark a tool as required. The requirement applies only when
the tool's trigger matches the current diff, so required workflow tools do not
create evidence gaps on source-only PRs.

```toml
[tools.actionlint]
required = true
```

### Rust unsafe evidence stack

For Rust repositories with an unsafe surface, `unsafe-review` is the third
static evidence pillar beside `cargo-allow` and `ripr`:

| Tool | Review question |
|---|---|
| `cargo-allow` | Is this exception owned, scoped, evidenced, and not silently broadened? |
| `ripr` | Does changed behavior appear exposed to a meaningful oracle? |
| `unsafe-review` | Does changed unsafe code have reviewable safety evidence? |
| `cargo-mutants` | Do tests fail against concrete mutants? |
| Miri | Does this concrete execution hit UB? |
| Codecov | Did this code execute? |

`unsafe-review` asks whether an unsafe change has the safety contract,
precondition guard, layout/alignment witness, aliasing/lifetime evidence, local
test reach, and witness route needed for credible review. It is advisory by
default and does not claim to prove soundness, UB-free status, or Miri
cleanliness. See [docs/UNSAFE_REVIEW_POLICY.md](docs/UNSAFE_REVIEW_POLICY.md)
and [docs/ci/unsafe-review.md](docs/ci/unsafe-review.md) for reusable repo
guidance.

## Review posting

`ub-review run` prepares evidence and review artifacts. `ub-review post` submits
`review/github-review.json` as one GitHub Pull Request Review:

```bash
ub-review run --posting review --out target/ub-review
ub-review post --review-json target/ub-review/review/github-review.json
```

`post` writes separate success/error receipts. Current `gate_outcome` is not
recomputed from them: `publication-result=posted` presently means the run
prepared a payload, not that GitHub confirmed it. Candidate-head posting has
retained proof; synthetic merge-result delivery binding remains open. See the
[product state](docs/PRODUCT_STATE.md#capability-matrix).

`ub-review gate-check` enforces a previously recorded gate verdict with the
same `fail-on-gate` resolution `run` uses (`auto` enforces only for
`--mode intelligent-ci`). The GitHub action's final `Enforce gate outcome`
step calls it instead of re-implementing that logic in bash:

```bash
ub-review gate-check \
  --gate-outcome target/ub-review/review/gate_outcome.json \
  --fail-on-gate auto \
  --mode intelligent-ci
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
| `config` | empty | Optional repo-local or absolute TOML config path; overrides `preset` when set. |
| `profile` | `gh-runner` | Box profile. |
| `base` | `origin/main` | Base ref. |
| `head` | `HEAD` | Head ref. |
| `out` | `target/ub-review` | Packet output directory. |
| `tool-bundle` | `core` | `none`, `core`, `bun-fast`, or `full`. |
| `install-tools` | `true` | Best-effort sensor install. |
| `setup-rust` | `true` | Select Rust 1.95 with rustup when available. |
| `install-mode` | `auto` | `auto`, `release`, `source`, or `path`. |
| `binary-path` | empty | Existing binary path for `install-mode=path`. |
| `release-version` | empty | Release tag for release downloads; empty lets tagged action refs provide the tag. |
| `release-asset` | `ub-review-x86_64-unknown-linux-gnu.tar.gz` | Linux x64 release archive asset. |
| `allow-heavy` | `false` | Permit heavy witness classes. |
| `posting` | `review` | `review` posts one Pull Request Review; `artifact-only` only writes files. |
| `mode` | `review-byok` | BYOK grouped review mode. `intelligent-ci` selects the required-gate product mode; legacy `review-direct` is accepted as an alias. |
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
| `fail-on-gate` | `auto` | Gate enforcement: `auto`, `true`, or `false`. The action's final `Enforce gate outcome` step runs `ub-review gate-check`, which fails the check when `review/gate_outcome.json` records a `fail` conclusion and enforcement resolves to `true`; artifacts, the job summary, and PR review posting always complete first. `auto` resolves to `true` for `mode=intelligent-ci` and `false` otherwise. |
| `github-summary` | `true` | Append running summary to job summary. |

## Repo Config Proof Policy

Custom configs can require proof in `intelligent-ci` mode. Matched requests are
still routed through the central proof broker allowlist and runtime budget.

```toml
review_profile = "bun-ub-v0"
profile = "gh-runner"

[repo]
kind = "rust"

[[proof.required]]
id = "cargo-check"
languages = ["rust"]
diff_classes = ["source-general", "source-ub"]
command = "cargo check --workspace --locked"
reason = "Required Rust workspace check for intelligent CI."
cost = "focused-build"
timeout_sec = 300
required = true
```

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
| `gate-outcome-path` | Deterministic gate verdict `review/gate_outcome.json`. |
| `gate-conclusion` | Legacy single verdict — `pass`, `fail`, or `inconclusive`. Unchanged in meaning; this is what enforcement acts on. |
| `analysis-result` | What the investigation established: `clean`, `findings`, `limited`, or `not_proven`. Insufficient evidence never reports `clean`. |
| `publication-result` | Run-stage projection: `posted` currently means a review payload was prepared, not GitHub-confirmed. Post success/failure receipts remain separate until #959/#960 finalization. Other values are `not_needed`, `failed`, or `not_proven`. |
| `gate-result` | Truthful check verdict: `pass`, `finding`, or `not_proven`. May be `not_proven` while `gate-conclusion` is `pass` — enforcement is unchanged, the report is not. |
| `not-proven-reasons` | JSON array of token-prefixed reasons any result is `not_proven`. Consume with `fromJSON`. |
| `sensor-coverage` | JSON object of instrument coverage counts. Consume with `fromJSON`. |

## Bootstrap note

With `install-mode=auto`, tagged action refs first try the Linux x64 release
archive and fall back to a source build when the asset is unavailable. Commit
SHA refs use the source build path. This keeps first adoption token-free and
mechanically simple while leaving the faster release-binary path available for
tagged rollouts. Explicit `install-mode=release` is strict: missing archives,
missing checksum receipts, checksum mismatches, and unsupported runners fail
instead of rebuilding from source. Use `auto` when fallback is acceptable. The
consuming workflow can cache Cargo registry and target directories if needed.

## Codex lane notes

Codex work should follow [docs/CODEX_FINISH.md](docs/CODEX_FINISH.md): one
small green PR at a time, MiniMax M3 primary for v0, GLM skipped until
approved, agent harnesses out of the hot path, and real sensor defects filed in
the matching `*-swarm` repo instead of silently absorbed into `ub-review`.

## Product state and implementation order

[docs/PRODUCT_STATE.md](docs/PRODUCT_STATE.md) is the canonical earned-state
matrix. It distinguishes implemented, production-wired, retained-run,
trusted-authority, and externally calibrated capability instead of treating a
schema or merged type as live authority.

[Issue #945](https://github.com/EffortlessMetrics/ub-review/issues/945) owns the
PR-sized execution order. Parent issues retain capability contracts; retained
packets and exact-head verification outrank both prose surfaces. Historical
roadmap/spec documents remain useful for design intent, but they are not the
current merge-front authority.

Fast/late sensor shadowing completed in #1263/#955. The live authority migration
is deliberately serial:

```text
#1266 / #956  proof and worker execution -> shadow TaskLedger
  -> #957     projection reconciliation
  -> #958     pure FinalizedOutcome
  -> #959     delivery finalization
  -> #960     shadow integration and verification
  -> #962     complete Horizon A packet proof
```

[The model-off CI-efficiency path](docs/CI_EFFICIENCY_PATH.md) is a bounded
route through the same architecture, not a competing roadmap. Output
containment through #1269/#1283 precedes measured use. #1274 proves the frozen
model-off `SharedRunPlan` core and unlocks the pure #986 then deterministic #987
scheduler path; #1120 remains required before model-programme and final-lead
augmentation. Measured #1275 acceptance and #1277 result-plane
non-interference precede #1015, which is the later enforcement switch. Until
then, legacy `gate_outcome.conclusion` remains authority.

Do not jump from the current substrate past those prerequisites into live
scheduler authority, trusted learning, provider-native orchestration,
CI-advisor behavior, or sole-gate promotion.

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

# Repo operating handoff

This handoff packages the repo-operating work landed from the web-Codex PR
queue. Use it to apply the same baseline to other Rust repos and to hand the
remaining product/tool work back to the correct lanes.
For a copyable adoption guide, use [PORTING_BASELINE.md](PORTING_BASELINE.md).

## Landed layers

- PR #154: `cargo xtask precommit` fast lane with affected-package routing,
  changed-line Clippy gating, diff artifacts, real `ripr check --diff` and
  `unsafe-review check --diff` calls, and missing-tool receipts.
- PR #158: `resolved-tools.json` and `tool-status.json` artifacts with root and
  `review/` copies, status/exit/timeout fields, and compatibility for older
  sensor receipts.
- PR #160: concurrent model/proof resource doctrine. Provider wait does not
  occupy local CPU/disk proof leases, but every pass still obeys the configured
  runtime timeout.
- PR #162: proof-planner artifacts before proof execution:
  `review/proof_planner_input.json`, `review/proof_planner_output.json`, and
  root `proof_tasks.ndjson`.
- PR #198: standard runner image docs now treat `UB_REVIEW_TOOL_DIR` as an
  install prefix and put `$UB_REVIEW_TOOL_DIR/bin` on `PATH`.
- PR #199: Bun artifact verification requires and cross-checks
  `resolved-tools.json` and `tool-status.json` at the root and under `review/`.
- PR #200: `cargo xtask precommit` clears `target/precommit` before writing new
  receipts and uses workspace-wide `cargo fmt --all -- --check`.
- PR #201: coverage sensor runs write `sensors/coverage/status.json`, and the
  verifier checks the coverage status receipt.
- PR #203 and follow-up pin correction: standard-image
  `doctor --require-core-tools` fails when `tokmd` drifts from the pinned
  published version.
- PR #204: Bun artifact verifier rejects inline review boilerplate and CI runs
  the verifier self-test.
- PR #264: model-stage artifacts record primary, proof-planner, refuter, and
  follow-up model stages; `review/final_compiler_input.json` records the final
  compiler inputs after follow-up evidence.
- PR #265: follow-up model-stage test coverage now exercises provider,
  duration, HTTP status, response shape, and cache-token fields.
- PR #267: `follow_up_results` and `follow_up_outputs` preserve disposition,
  evidence need, candidate ids, and observation group ids from orchestrator
  follow-up tasks, and the verifier cross-checks that provenance.
- PR #270: `resolved_candidates` records candidate disposition after follow-up
  outputs, and the verifier rejects open follow-up observations being promoted
  into resolved dispositions.
- Coverage sidecar receipt layer: the local coverage sensor now writes
  `coverage-summary.json`, `changed-lines.json`, and `upload.json` sidecars, and
  the verifier checks that each stays execution telemetry rather than a
  correctness claim.
- PR #274: `ub-review doctor` reports MiniMax and OpenCode provider-key env
  presence without printing secret values, and Bun runner docs map GitHub
  secrets to `UB_REVIEW_*` env vars.
- PR #275: `work_queue.json` records baseline sensor tasks before proof-planner
  tasks, and `tool-status.json` carries timeout, artifact-budget, and lease
  metadata so the verifier can audit queue/tool parity.
- PR #276: the artifact verifier cross-checks `tool-status.json` against
  `resolved-tools.json`, and generated model artifact path segments are bounded
  with stable hash suffixes instead of trusting model-sized ids as filenames.
- PR #277: `tool-gate-outcomes.json`, `review/tool-gate-outcomes.json`, and
  `tool_gate_outcomes.ndjson` record per-tool gate policy outcomes separately
  from sensor status.
- PR #278: malformed `gate-decision.json` receipts stay visible as malformed
  evidence, and the verifier rejects impossible tool-gate pass/fail claims.
- PR #279: the self gate no longer subscribes to `pull_request.synchronize`;
  opened and ready-for-review remain the default review-producing passes.
- PR #282: the artifact verifier treats raw `OPENCODE` assignments as secret
  leaks while allowing GitHub secret placeholders, and its observation
  artifact-only routing matches the Rust orchestrator for verifier/self-test
  meta noise.
- PR #283: `work_queue.json` and `work_events.ndjson` record
  `initial_packet_status` so the first packet can distinguish ready receipts,
  pending initial work, pending late follow-up work, and non-initial tasks.
- PR #284: `docs/ci/work-queue.md` records the packet-status contract for
  initial, late, adaptive, artifact-only, and gate-only work.
- PR #285: the artifact verifier treats raw `FACTORY_API_KEY` assignments as
  secret leaks while allowing GitHub secret placeholders, including escaped
  placeholders, and keeps verifier-script meta chatter artifact-only.
- PR #286: the shared context packet includes initial work-queue state so lanes
  see ready receipts and pending proof before they reason about the diff.
- PR #287: proof receipt routing artifacts record which proof receipts route to
  which follow-up consumers.
- PR #288: `review/final_orchestrator_plan.json` captures the post-follow-up
  convergence plan used for the final compiler pass.
- PR #289: resolved final proof follow-ups are removed from the remaining final
  task set instead of leaking stale work into the final decision.
- PR #290: `review/metrics.json` records the final follow-up task count after
  late receipt routing and final convergence filtering.
- Bun PR #49: the Bun gate is pinned to
  `EffortlessMetrics/ub-review@804d198b5a15a0df94bb4f43750dba71165916cd` with
  a successful `UB evidence packet / gh-runner` run, terminal state
  `sufficient`, artifact-only post skip, and verifier pass.

## Adoption checklist

For a serious Rust repo, copy the baseline in this order:

1. Add `AGENTS.md` with review-fast PR rules, validation expectations, and
   ownership boundaries.
2. Add `docs/REPO_STYLE.md` and policy docs that define tool layering:
   `cargo-allow`, `ripr`, `unsafe-review`, `ast-grep`, actionlint, Codecov,
   cargo-mutants, Miri, sanitizers, and `xtask`.
3. Add `policy/allow.toml`, `policy/ci-budget.toml`,
   `policy/ci-lanes.toml`, and `policy/ci-risk-packs.toml`.
4. Add `cargo xtask policy-check`, `policy-inventory`, and `precommit`.
5. Emit `resolved-tools.json`, `tool-status.json`, planner artifacts, proof
   requests, proof receipts, resource leases, metrics, and review artifacts.
6. Enforce the PR body split: reviewer-value findings/questions/proof only in
   the PR review; logs, lane rosters, status tables, and raw output in artifacts.
7. Wire CI so draft and ready-for-review passes get the configured proof lease;
   do not spend a full runner on every synchronize event unless the repo opts in.
8. Pin consuming workflows to a verified commit SHA. Move the pin only after
   local verifier checks and a consumer packet smoke succeed.

## Review gate defaults

Use these defaults unless a repo opts into stricter behavior:

- local precommit runs `fmt`, affected-package `check`, affected-package
  Clippy on changed lines, and relevant static receipts;
- missing `cargo-allow`, `ripr`, `unsafe-review`, `actionlint`, or `ast-grep`
  is missing evidence, not a clean result;
- configured tool gates write explicit pass, fail, not-evaluated, or
  missing-evidence outcomes; sensor success alone is not a gate pass;
- on standard runner images, missing core tools or a stale `tokmd` pin are
  image drift and should fail `doctor`;
- `cargo-allow` owns source-tree exceptions;
- `ripr` owns static mutation-exposure signal;
- `unsafe-review` owns unsafe/native reviewability;
- `xtask` calls tools and normalizes receipts; it does not fork tool logic;
- Codecov is execution-surface telemetry, not correctness proof;
- runtime proof is routed by risk and claim.

## Validation package

Before merging a repo-operating PR, record:

```text
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
cargo xtask policy-check
cargo xtask policy-inventory
cargo xtask precommit
TOML parser sweep over policy/runtime/config/profile files
git diff --check
```

For docs-only changes, the same full gate is preferred when cheap; otherwise
record exactly which checks were skipped and why.

## Handoff to lanes

- Product/provider lane: PR #153 was closed out of this lane. It contains
  MiniMax prompt-prefix caching ideas, failed CI, and should be rebuilt only if
  Steven explicitly prioritizes provider-cache economics.
- Docs lane: PR #156 was closed as superseded by #154 and #158. If its docs
  concepts are still useful, reintroduce them as a docs-only PR.
- Runner-image lane: local validation still observed missing `cargo-allow` and
  `ast-grep`. Runner images should install those tools so precommit and CI
  receipts become stronger by default.
- Tool-owner lanes: file real `cargo-allow`, `ripr`, `unsafe-review`, `tokmd`,
  or `ast-grep` defects in the matching tool/swarm repo with reproduction
  artifacts. Do not patch around tool bugs inside `ub-review`.

## Anti-patterns

- Do not merge several partial doctrine PRs that say the same thing.
- Do not turn missing evidence into a pass.
- Do not post setup tables, lane rosters, command logs, or generic
  no-finding prose in PR review bodies.
- Do not let inline comments carry the boilerplate banned from PR bodies.
- Do not reimplement external tool internals in `xtask` or `ub-review`.
- Do not treat model provider wait as local CPU proof work, and do not exceed
  the runtime hard timeout.

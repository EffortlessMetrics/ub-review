# UB-REVIEW-SPEC-0002 — review-byok surface

Status: authored 2026-06-06 (release surface spec wave, docs-only).
Child of UB-REVIEW-SPEC-0001. Documents the current behavior of
`mode=review-byok`; contract intent is marked as intent. Maturity per the
umbrella: production — the Bun consumer pin is live
(`EffortlessMetrics/ub-review@804d198b...`, README "Copy/paste Bun setup").

## Purpose

review-byok is the adoption surface: a repo adds the GitHub Action, supplies
its own model key, and gets one grouped, evidence-backed PR review per
qualifying pass. It is non-blocking by default — the CodeRabbit-shaped entry
point, except the review is compiled from a deterministic evidence packet and
every gap is recorded as a gap. The same run also writes the full artifact
packet and a truthful `gate_outcome.json`, so a repo that later wants the
`intelligent-ci` gate (spec 0003) flips enforcement, not tooling.

## User question

```text
Can I get useful AI PR review with my own model key and minimal setup?
```

## Lifecycle moment

`pull_request` events on the consumer repo. The shipped consumer default is
two posting passes: draft PR `opened` gets the first packet, `ready_for_review`
gets the second (README workflow trigger; docs/ACTION_CONSUMER_BUN.md). A repo
may also trigger on `synchronize`/`reopened`; those passes run the full
pipeline but post nothing unless the profile lists the event in
`[gate].post_review_on` (default `["opened", "ready_for_review"]`,
src/config.rs; honored for synchronize/reopened since PR #304, proven quiet on
PR #307 runs).

## Consumer

- a repo maintainer pasting one workflow and one secret;
- the PR author and human reviewers reading the grouped review and the job
  summary;
- automation downloading the uploaded packet artifact (full contract in spec
  0004).

## Inputs

Minimal action usage (README "Copy/paste Bun setup"; all inputs in action.yml):

```yaml
- uses: EffortlessMetrics/ub-review@<full-commit-sha>
  with:
    preset: bun-ub            # default; the only production preset today
    posting: review           # default
    mode: review-byok         # default
    github-token: ${{ github.token }}
    minimax-api-key: ${{ secrets.MINIMAX_API_KEY }}
```

Provider secrets:

- `MINIMAX_API_KEY` is the primary path. The action forwards it as
  `UB_REVIEW_MINIMAX_API_KEY` (action.yml run step; src/main.rs
  `model_api_key_env`). MiniMax M3 over the Anthropic-compatible endpoint is
  the default lane model, with explicit prompt caching on that endpoint only.
- `OPENCODE` / `OPENCODE_API_KEY` is optional, forwarded as
  `UB_REVIEW_OPENCODE_API_KEY`. Its role depends on `provider-policy`
  (src/cli.rs `ModelProviderPolicy`, src/main.rs
  `provider_spec_for_lane_with_key_state`):
  - `minimax-primary` (action default): MiniMax on all lanes; when the
    OpenCode key is present the `opposition` lane runs as an OpenCode canary
    with MiniMax as its fallback. Non-opposition lanes have no fallback spec
    under this policy.
  - `primary-with-fallback`: every MiniMax lane gets an OpenCode fallback
    spec — this is the policy that makes preflight fallback and the runtime
    retry available on every lane.
  - `minimax-only`: OpenCode ignored entirely. The README Bun consumer
    workflow uses this.
- `FACTORY_API_KEY` is explicitly NOT part of this path. It is not an action
  input (docs/GH_RUNNER_BUN.md), doctor does not check it
  (docs/RUNNER_IMAGE.md), and the artifact verifier guards it as a secret
  value name — a raw `FACTORY_API_KEY` assignment in the packet fails
  verification (scripts/verify-bun-review-artifacts.py `SECRET_VALUE_NAMES`).

Default lanes are diff-class adaptive (src/main.rs `review_lanes_for_width`,
`default_lanes_for_diff_class`; the six builtin base lanes in
src/builtin.rs, the widened 10/20 and non-UB diff-class lane sets in
src/main.rs):

```text
source-UB diffs      ub, source-route, tests, arch, opposition, security
                     (six builtin lanes, all MiniMax-M3; lane-width 10/20
                     widens this MiniMax lane set)
general source diffs correctness, tests-red-green, source-route,
                     architecture, ... (no source-UB assumptions)
tests-only diffs     tests-focused lane set
workflow/tooling     workflow-focused lane set
docs-only diffs      zero model lanes — no model spend
```

Tuning defaults (action.yml): `lane-width` 10, `model-concurrency` 8,
`max-model-calls` 14, `model-timeout-sec` 300, `max-inline-comments` 8,
`review-body-max-bytes` 60000. `depth` quick/standard/deep maps to lane width
6/10/20 (src/cli.rs `ReviewDepth`).

Sensor install is `install-tools: true` + `tool-bundle: core` by default
(tokmd, cargo-allow, ripr, unsafe-review, ast-grep, actionlint;
scripts/install-gh-runner-tools.sh). Sensors are advisory in this mode; a
missing binary is an evidence gap, not an error.

## Output artifact / user surface

Visible in the PR:

- exactly one grouped PR review per posting pass, submitted by the separate
  `post` step. `run` only writes artifacts; `post` submits
  `review/github-review.json` (docs/ARCHITECTURE.md). Never per-lane comments,
  issue comments, or status chatter.
- at most `max-inline-comments` (default 8) inline comments, each validated
  against the line-mapped diff before posting.
- no-LGTM posture: the review event is always `COMMENT` — never `APPROVE` or
  `REQUEST_CHANGES` — and `post` refuses any payload whose event is not
  `COMMENT` (src/main.rs cmd_post). `ban_standalone_approval` defaults to true
  (src/config.rs); lane prompts carry the no-LGTM posture text (src/main.rs
  `NO_LGTM_POSTURE`); the verifier rejects standalone approval lines.
- boilerplate suppression: the review body carries only decision, findings,
  verification questions, proof results, refutations, parked follow-ups, and
  specific evidence gaps (docs/REVIEW_BODY_CONTRACT.md). No lane rosters,
  provider/sensor tables except on failure, or "no issues found" filler — the
  verifier bans those phrases outright.
- when the suppressor withholds a no-value body, nothing posts and
  `review/github-review-skip.json` records why. `[review_body]
  summary_only_body` controls the posture when summary-only findings exist:
  `suppress` (default), `post_substantive`, `post_all` (src/config.rs
  `SummaryOnlyBodyPolicy`); the skip receipt records the policy key.
- the job summary: `running-summary.md` is appended to
  `GITHUB_STEP_SUMMARY` by default (`github-summary: true`).

Artifact-only (full contract in spec 0004): the packet under `out`
(default `target/ub-review`) — `input/`, `sensors/`, `lanes/`,
`observations/`, `candidates/`, `review/` (including `review.md`,
`review.json`, `metrics.json`, `gate_outcome.json`, proof receipts),
`events.ndjson`, `running-summary.md`. The action names the load-bearing
paths as outputs, including `gate-outcome-path`, `github-review-path`,
`summary-path`, and the post receipts (action.yml outputs).

## Required fields

```text
review/github-review.json        event == "COMMENT"; body within
                                 review-body-max-bytes; comments validated;
                                 optional suggestion is allowed only on
                                 unsafe-review comments with bounded concrete
                                 replacement text
review/github-review-skip.json   XOR with github-review.json; status one of
                                 skipped_empty_smoke |
                                 skipped_artifact_only_body |
                                 skipped_pass_policy |
                                 skipped_gate_failure_artifact_only
                                 (src/main.rs; failed artifact-only gates
                                 use this status, never skipped_empty_smoke)
review/gate_outcome.json         schema ub-review.gate_outcome.v1;
                                 conclusion "pass" | "fail"; reasons carry
                                 receipt pointers
review/terminal_state.json       status: sufficient | artifact-only |
                                 needs-reviewer-attention | failed-to-review
model lane receipts              provider, model, endpoint_kind, status
                                 (ok | degraded | missing_key | failed |
                                 invalid_json | timed_out | rate_limited |
                                 auth_failed | bad_envelope |
                                 preflight_failed | skipped), fallback_from;
                                 skipped = budget exhausted before execution,
                                 still a recorded evidence gap
running-summary.md               must include a "Missing evidence" section
                                 (verifier-required heading)
```

## Advisory vs blocking behavior

In review-byok everything is advisory by default. `fail-on-gate` resolves
`auto` → false for this mode (src/cli.rs `FailOnGate::resolved`; only
`intelligent-ci` auto-resolves true). The action's run step always passes
`--fail-on-gate false` so artifacts, the job summary, and posting complete;
enforcement lives solely in the final `gate-check` step, which in byok mode
passes regardless of the gate conclusion unless the repo sets
`fail-on-gate: 'true'` explicitly (action.yml "Enforce gate outcome").

`gate_outcome.json` is still written truthfully on every run — required-sensor
evidence gaps that would block in intelligent-ci mode are advisory here
(src/main.rs gate outcome construction). Model findings are never proof and
never feed the gate verdict (umbrella boundary).

`fail-on-post-error` defaults to false: a failed review submission writes
`post-error.json` and does not fail the job.

## Fail-closed behavior

- Missing keys are missing evidence, never clean evidence. A lane without its
  provider key gets receipt status `missing_key` (src/main.rs), counts as a
  model evidence issue, and appears under "Missing evidence" in
  running-summary.md. `doctor` reports each provider env as present/missing
  before any run (src/main.rs cmd_doctor).
- Preflight fallback: provider preflight runs before lane fan-out;
  `selected_provider_spec` picks the lane's fallback spec when the primary
  fails preflight. `preflight_failed` is terminal only when no usable
  fallback exists (src/main.rs).
- Runtime fallback retry (landed in PR #315): a lane that fails mid-run with
  `rate_limited`, `timed_out`, or HTTP 5xx gets one bounded retry on its
  fallback spec, inside the `max-model-calls` budget, receipted via
  `fallback_from`; the evidence issue is recorded only on terminal failure
  (src/main.rs `runtime_fallback_retry_spec`). Honest constraint: the retry
  needs a fallback spec to exist — under the default `minimax-primary` policy
  only the opposition canary has one; `primary-with-fallback` extends it to
  every lane. The remainder of #310 is open: per-provider `max_concurrency`
  enforcement, 429 backpressure, and `[providers]` config parsing (the
  deliberately reserved section, spec 0006).
- Model and provider failures never block. They degrade the review surface
  (terminal status may reach `failed-to-review`) but never redden the gate;
  the gate check itself recognizes exactly the string `pass` and treats
  anything else as fail — which in byok default mode is informational, not
  job-failing.
- Posting on quiet passes fails closed into silence: a pass whose event is
  not in `post_review_on` writes `github-review-skip.json` with
  `skipped_pass_policy` instead of posting. Legacy
  `[gate].synchronize_mode` is stripped with a deprecation `PolicyError`
  (#306); do not configure it expecting behavior.

## Trust boundary / non-claims

```text
models investigate; they never prove
a posted review is evidence-backed opinion, not a correctness verdict
review-byok does not gate; the gate exists but is opt-in here
missing keys, missing sensors, failed lanes are recorded gaps, never silence
no approval is ever issued; zero-finding reviews must show their work
```

Scope honesty: `bun-ub` is the only production preset; lane prompts and the
tokmd analyze preset are Bun/UB-tuned (`TOKMD_ANALYZE_PRESET` is hardcoded
`bun-ub`, src/main.rs). Diff-class lane adaptation gives non-UB diffs general
correctness lanes, so the surface runs on any repo — but "useful review on
arbitrary non-Bun repos" is contract intent, not a proven claim; the proven
claim is the Bun consumer pin. A previously known quality gap — follow-up
prompts routing receipt pointers instead of receipt content (#311) — is
closed by PR #322: packets now embed bounded command-output tails.

The six reliance questions:

```text
Rely on:     one grouped COMMENT review max per posting pass; bounded,
             diff-validated inline comments; no approvals ever; full packet
             artifacts; gaps recorded as gaps.
Break gate:  nothing, unless the repo sets fail-on-gate: 'true'.
Advisory:    everything — sensors, model findings, gate_outcome.json.
PR-visible:  the grouped review body + inline comments; the job summary.
Artifact:    lanes, observations, candidates, proof receipts, metrics,
             gate outcome, events timeline, skip receipts.
Ten minutes: paste the README workflow, add MINIMAX_API_KEY, open a draft
             PR; the pass builds the packet, posts the grouped review (or a
             truthful skip receipt), uploads the artifact, and the job
             summary shows lane and evidence status. No key yet? The run
             still completes — sensors and packet only, model lanes recorded
             as missing evidence.
```

## Validation commands

```bash
ub-review doctor --profile gh-runner          # tools + provider env present/missing
cargo run -- run --config target/ub-review-smoke.toml --profile gh-runner \
  --base HEAD --head HEAD --dry-run --out target/ub-review-smoke
                                              # full dry-run path: scripts/smoke-local.sh
python scripts/verify-bun-review-artifacts.py target/ub-review \
  --min-ok-model-lanes 10 --require-no-model-evidence-failures
                                              # packet contract incl. no-LGTM + secret hygiene
python scripts/verify-bun-review-artifacts.py --self-test
```

## Implementation PR slices

This spec is docs-only; it routes open work, it does not add any:

1. #310 remainder — per-provider concurrency, 429 backpressure, `[providers]`
   parsing (lands under spec 0006).
2. #311 — include proof receipt content in follow-up prompts (landed in
   PR #322).
3. #306 — DONE: `[gate].synchronize_mode` was deleted with a deprecation
   `PolicyError`; `post_review_on` is the only posting policy.
4. Non-Bun quickstart: a documented ten-minute path for repos that are not
   Bun-shaped (preset/tokmd-preset generalization). New slice; no issue yet.

## Release note claim

```text
ub-review can run BYOK review lanes over prepared evidence:
one grouped, non-blocking PR review per pass with your own MiniMax key,
validated inline comments, no LGTM noise, and every missing input
recorded as missing evidence.
```

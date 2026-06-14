# UB-REVIEW-SPEC-0006 - provider, cache, and fallback surface

Status: authored 2026-06-06 (release surface spec wave, docs-only).
Child of UB-REVIEW-SPEC-0001. Documents the current behavior of model
provider selection, prompt-prefix caching, and fallback; contract intent is
marked as intent. Maturity per the umbrella: partial - the CLI-flag surface
is production (preflight fallback, runtime fallback retry, prompt caching on
by default); `[providers].policy`, per-provider `max_concurrency`, and
retryable-failure wave shedding are parsed and executed, while provider
model/env/role/prompt-cache config remains open.

## Purpose

This surface answers how a repo points ub-review at model providers without
surprises: which keys exist, which provider serves which lane, what happens
when a provider rate-limits or dies mid-run, and how prompt caching keeps a
ten-lane review affordable. The design center is BYOK with honest receipts:
every provider interaction - preflight, lane call, fallback, retry, cache
hit - leaves a receipt, and no provider failure ever becomes a gate failure
or silent clean evidence.

## User question

```text
How do I configure model providers safely and cheaply?
```

## Lifecycle moment

Two moments:

- adoption time: the maintainer sets repository secrets and the
  `provider-policy` / model action inputs (action.yml);
- every run's model phase: lane assignments resolve provider specs
  (src/main.rs `model_assignments`), provider preflights run once per
  distinct spec, then the wave loop fans lanes out under the global
  concurrency and call budget (src/main.rs
  `run_available_model_lanes_with_runner`).

Provider behavior is identical across run passes; posting policy (spec 0002)
is a separate axis.

## Consumer

- a repo maintainer choosing keys and a provider policy in one workflow file;
- the run itself, which selects a spec per lane and degrades honestly when a
  provider is missing or failing;
- automation and humans reading provider receipts
  (`review/provider-preflight-status.json`, model lane receipts in
  `review/review.json`, `review/cache_events.ndjson`, `review/metrics.json`);
- a cost-conscious operator reading cache token counts to verify the prefix
  cache is actually being hit.

## Inputs

Provider identities and key contract:

- MiniMax is the primary provider (`ModelProvider::MiniMaxDirect`, key
  `minimax`, src/main.rs). Secret `MINIMAX_API_KEY` flows through the
  `minimax-api-key` action input to the `UB_REVIEW_MINIMAX_API_KEY` env var
  (action.yml; src/main.rs `model_api_key_env`). Defaults: model `MiniMax-M3`
  (src/cli.rs `minimax_model`), endpoint kind `anthropic` (src/cli.rs
  `minimax_provider_kind`), which maps to the Anthropic-messages endpoint
  `https://api.minimax.io/anthropic/v1/messages` with `X-Api-Key` auth;
  `openai` selects `https://api.minimax.io/v1/chat/completions` with bearer
  auth (src/main.rs `model_api_url`, `model_auth_header`).
  `UB_REVIEW_MINIMAX_API_URL` overrides the URL.
- OpenCode is the fallback/canary provider (`ModelProvider::OpenCodeGo`, key
  `opencode-go`). Secret `OPENCODE` flows through `opencode-api-key` to
  `UB_REVIEW_OPENCODE_API_KEY`. The CLI default `opencode-model` is
  `minimax-m3`; this repo's gate workflow sets `mimo-v2.5`
  (.github/workflows/ub-review-gate.yml). Fast lanes under the wide policy
  hardcode `deepseek-v4-flash` (src/main.rs `opencode_flash_spec`).
  `opencode-endpoint-kind` defaults to `auto`, which resolves `deepseek-*`,
  `kimi-*`, and `mimo-*` models to openai-chat and everything else to
  anthropic-messages (src/main.rs `resolve_opencode_endpoint_kind`).
- `FACTORY_API_KEY` is explicitly excluded from this surface. It is not an
  action input (docs/GH_RUNNER_BUN.md), and the artifact verifier treats it
  as a secret value name - a raw `FACTORY_API_KEY=...` assignment anywhere in
  the packet fails verification (scripts/verify-bun-review-artifacts.py
  `SECRET_VALUE_NAMES`).

Provider policy (`provider-policy` input, default `minimax-primary`;
src/cli.rs `ModelProviderPolicy`; semantics in src/main.rs
`provider_spec_for_lane_with_key_state` and
`fallback_provider_spec_for_lane`):

```text
minimax-primary (default; `auto` is an alias with identical arms)
    MiniMax on every lane. When the OpenCode key is present, the
    `opposition` lane runs as an OpenCode canary with MiniMax as its
    fallback spec. No other lane has a fallback spec.
primary-with-fallback
    MiniMax on every lane; every lane gets an OpenCode fallback spec when
    the OpenCode key is present (flash model for fast lanes, the canary
    model otherwise). This is the only policy that arms preflight fallback
    and the runtime retry on every lane. This repo's own gate uses it
    (.github/workflows/ub-review-gate.yml).
minimax-only
    OpenCode ignored entirely. The README Bun consumer workflow uses this.
opencode-go-canary
    Opposition lane on OpenCode unconditionally (missing key surfaces as
    missing_key at run time), MiniMax as its fallback; all other lanes
    MiniMax with no fallback.
opencode-go-wide
    Fast lanes (`*-fast`, `refute-finding-*`, `summary-pressure`,
    `duplicate-noise-filter`; src/main.rs is_opencode_fast_lane) on
    deepseek-v4-flash; all other lanes MiniMax; no fallback specs.
```

Concurrency and budget inputs: `model-concurrency` (default 8) is a global
per-wave in-flight cap; `max-model-calls` (default 14) is the total call
budget shared by first attempts and retries (src/cli.rs; src/main.rs wave
loop). `[providers.minimax].max_concurrency` and
`[providers.opencode].max_concurrency` cap each provider inside a wave; the
effective provider cap is also bounded by the global `model-concurrency`.
Invalid or zero values are stripped with `PolicyError` receipts. A provider
that returns `rate_limited`, `timed_out`, or HTTP >= 500 during a lane call
sheds the next scheduling wave to a healthy fallback for pending lanes where
one is configured; lanes without fallback still fail terminally as missing
model evidence instead of spinning.

Partly wired - the `[providers]` config section: `.ub-review.toml` on this
repo declares `policy = "primary-with-fallback"`,
`[providers.minimax]` (env, model, role, `prompt_cache =
"explicit-anthropic"`, `max_concurrency = 12`), and `[providers.opencode]`
(models, `max_concurrency = 8`). `policy` is read when CLI/env provider
policy is `auto`, and each provider's `max_concurrency` is read by the
model-lane wave scheduler. Descriptive keys (`env`, `model`, `role`,
`models`) are still documentation of intent. `[providers.minimax].prompt_cache`
is executable only for the current
MiniMax cache modes: absent means the default `explicit-anthropic` behavior,
`"explicit-anthropic"` preserves that behavior, and `"off"` disables the
Anthropic cache-control marker on the shared-context prefix. Invalid MiniMax values, and any
`[providers.opencode].prompt_cache`, are stripped with PolicyError receipts.

## Output artifact / user surface

All provider state is artifact-only by default:

- `review/provider-preflight-status.json` - one receipt per distinct
  provider spec. Specs with a key present start `planned` and are executed
  with a minimal strict-JSON prompt; missing keys are receipted as
  `missing_key` without any network call (src/main.rs
  `provider_preflight_receipt_for_spec`, `run_provider_preflights`).
  Request/response bodies land under `review/provider-preflight/<spec>/`.
  When the spec supports caching, the preflight call carries the shared
  context as the cacheable prefix, so the preflight doubles as the cache
  warm for the run.
- model lane receipts (in `review/review.json` `model_lanes`) - provider,
  model, endpoint_kind, status, reason, duration_ms, http_status,
  response_shape, `fallback_from`, `cache_usage` (src/main.rs
  `ModelLaneReceipt`).
- prompt-cache artifacts in `review/` - `shared_context.md`,
  `shared_context_cache_block.md`, `shared_context_hash.txt`,
  `cache_manifest.json` (schema `ub-review.cache_manifest.v1`; records
  `explicit_cache_provider = minimax`, `explicit_cache_endpoint =
  anthropic-messages`, `cache_lifetime = provider-ephemeral`), and
  `cache_events.ndjson` (schema `ub-review.cache_event.v1`; one
  `shared_context_prepared` event plus per-call events with cache token
  counts) (src/main.rs shared-context cache writers;
  scripts/verify-bun-review-artifacts.py cache checks).
- `review/metrics.json` - aggregated `prompt_cache_*` token counts under
  `models` (src/main.rs metrics builder).
- `running-summary.md` - a "Provider preflights" section (verifier-required
  heading) and provider failures under "Missing evidence".

PR-visible surface: nearly nothing, by design. The bun-ub-v0 profile sets
`include_provider_table = "on_failure"` (profiles/bun-ub-v0.toml), so
provider status reaches the review body only when model evidence actually
failed; healthy fallback use stays in artifacts.

Caching mechanics: `model_cache_mode_for_args` returns
`explicit-anthropic-cache-control` for MiniMax + anthropic-messages when the
resolved MiniMax prompt-cache mode is `explicit-anthropic`, and
`not-supported` for every other provider/endpoint pair or when the resolved
mode is `off` (src/main.rs). On the caching path the shared context is sent
as a separate text block with `"cache_control": {"type": "ephemeral"}`
ahead of the lane prompt (src/main.rs `anthropic_user_content`); on
non-caching paths the prefix is plain-concatenated into the prompt
(`combined_model_prompt`). Response usage is parsed into `ModelCacheUsage`
(input_tokens, output_tokens, cache_creation_input_tokens,
cache_read_input_tokens). Caching is ON by default because absent
`[providers.minimax].prompt_cache` resolves to `explicit-anthropic` and the
default MiniMax endpoint kind is `anthropic`; choosing
`minimax-provider-kind: openai` or setting
`[providers.minimax].prompt_cache = "off"` disables it.

Fallback mechanics (current behavior):

- preflight-time: the wave loop calls `selected_provider_spec` per lane; if
  the primary's preflight is not `ok` and the lane has a fallback spec whose
  preflight is `ok`, the lane runs on the fallback with `fallback_from` set
  to the primary's label and the reason recording the primary's preflight
  status. If neither passes, the lane is `preflight_failed` and recorded as
  missing model evidence (src/main.rs `selected_provider_spec`, wave loop).
- runtime retry (landed in PR #315, the retry half of #310): a lane whose
  call fails with `rate_limited`, `timed_out`, or `failed` with HTTP >= 500
  gets exactly one retry on its fallback spec, provided the lane has not
  already been retried or fallen back and the fallback key is present
  (src/main.rs `runtime_fallback_retry_spec`). Retries are queued and drain
  first in the next wave, spend the same `max-model-calls` budget, and stamp
  `fallback_from` plus reason "completed after runtime fallback retry" on
  success. The model evidence issue is recorded only on terminal failure -
  or on budget starvation, where the end-of-loop sweep marks the lane
  `skipped` and classifies it as missing evidence (src/main.rs
  `is_model_skipped_evidence_issue`).
- wave shedding: the same retryable runtime failure marks the attempted
  provider as backed off for the next wave. Pending first-attempt lanes whose
  primary provider is backed off run on their preflight-ok fallback instead,
  with `fallback_from` set and success reason "completed after provider
  backpressure fallback" (src/main.rs
  `selected_provider_spec_with_backpressure`). This is a one-wave scheduling
  nudge, not a gate failure.
- honest constraint: the retry needs a fallback spec to exist. Under the
  default `minimax-primary` policy only the opposition canary has one, so
  runtime fallback and wave shedding effectively arm one lane;
  `primary-with-fallback` extends them to every lane.
- no fallback anywhere else: the proof-planner lane, follow-up passes, the
  orchestrator, and the refuter all pin `direct_minimax_spec` with no
  provider selection and no fallback - if MiniMax is unavailable they are
  receipted `preflight_failed` or `skipped_budget` (src/main.rs
  `run_proof_planner_model_lane`, follow-up spec construction).

## Required fields

```text
ProviderSpec.label()                 "provider:model:endpoint_kind";
                                     providers minimax | opencode-go;
                                     endpoint kinds openai-chat |
                                     anthropic-messages
ProviderPreflightReceipt             provider, model, endpoint_kind, status,
                                     reason, duration_ms, http_status,
                                     response_shape, cache_usage
ModelLaneReceipt                     lane, provider, model, endpoint_kind,
                                     status, reason, duration_ms,
                                     http_status, response_shape,
                                     fallback_from, cache_usage
status taxonomy                      ok | degraded | planned | running |
                                     skipped | skipped_budget | missing_key |
                                     preflight_failed | auth_failed |
                                     rate_limited | timed_out | invalid_json |
                                     bad_envelope | failed
                                     (classify_model_error maps 401/403/auth,
                                     429/rate, timeout, parse, assistant-
                                     content envelope, else failed)
ModelCacheUsage                      input_tokens, output_tokens,
                                     cache_creation_input_tokens,
                                     cache_read_input_tokens
cache_manifest.json                  schema ub-review.cache_manifest.v1;
                                     explicit_cache_provider = minimax;
                                     explicit_cache_endpoint =
                                     anthropic-messages;
                                     cache_lifetime = provider-ephemeral
cache_events.ndjson                  schema ub-review.cache_event.v1; at
                                     least one shared_context_prepared event
```

## Advisory vs blocking behavior

Everything on this surface is advisory. Provider and model failures -
including every status above, preflight failures, and fallback use - are
classified as model evidence issues (src/main.rs `is_model_evidence_issue`)
and surface under "Missing evidence"; they never appear among gate reason
kinds (`required-proof`, `tool-gate`, `required-sensor`, `blocking-finding`,
`policy`, `internal`; spec 0003) and never redden the gate (ADR 0002:
model/provider failures are in the never-red list). A run with zero working
providers still completes: sensors and the packet are built, model lanes are
receipted as missing evidence, and the terminal state degrades honestly
(`failed-to-review` at worst) without failing the job in review-byok mode.

Fallback use is not a finding. It changes lane receipts (`fallback_from`)
and, in the bun-ub-v0 profile, reaches the PR body only via the on-failure
provider table - that is, only when it is trust-affecting because model
evidence was actually lost.

## Fail-closed behavior

- Missing key is missing evidence, never clean evidence: the preflight
  receipt is `missing_key` without a network call, the lane receipt is
  `missing_key`, and doctor reports each provider env as present/missing
  before any run (src/main.rs `cmd_doctor`).
- Preflight gates lane spend: a lane whose primary and fallback both fail
  preflight never burns a model call; it is `preflight_failed` with the
  preflight reason attached.
- One retry, bounded: the retry guard refuses a second fallback
  (`already_retried || fallback_from.is_some()`), refuses non-transient
  classes, and refuses when the fallback key is absent (src/main.rs
  `runtime_fallback_retry_spec`). Retries cannot exceed `max-model-calls`;
  a starved retry is receipted `skipped` and counted as missing evidence.
- Secret hygiene fails the verifier: raw assignments of `MINIMAX_API_KEY`,
  `FACTORY_API_KEY`, `GITHUB_TOKEN`, and the other `SECRET_VALUE_NAMES`
  anywhere in the packet fail verification
  (scripts/verify-bun-review-artifacts.py).
- Misconfiguration cannot silently de-fang policy: unknown top-level config
  sections are stripped with `PolicyError` receipts; invalid
  `[providers].policy` and provider `max_concurrency` values are stripped
  with provider policy receipts. Descriptive provider keys remain tolerated
  because they are not executable behavior yet.

## Trust boundary / non-claims

```text
provider receipts describe availability and spend, not review quality
fallback use is an availability event, not a finding
prompt caching is a cost optimization with provider-ephemeral lifetime;
  no correctness or determinism claim attaches to a cache hit or miss
[providers].policy and provider max_concurrency are behavior
minimax prompt_cache is behavior; env/model/role keys remain documentation
  of intent
no claim of provider redundancy: default policy arms fallback on one lane,
  and proof-planner/follow-up passes have no fallback at all
```

The six reliance questions:

```text
Rely on:     UB_REVIEW_MINIMAX_API_KEY / UB_REVIEW_OPENCODE_API_KEY env
             contract; the provider-policy lane mapping above; preflight
             receipts before lane spend; one bounded runtime retry on
             rate_limited/timed_out/5xx where a fallback spec exists;
             per-provider max_concurrency caps within the global wave cap;
             cache_usage token fields in receipts and metrics; provider
             failures recorded as missing evidence.
Break gate:  nothing on this surface, ever.
Advisory:    all provider, cache, and fallback state.
PR-visible:  provider tables only on failure (bun-ub-v0 policy); otherwise
             nothing.
Artifact:    provider-preflight-status.json, lane receipts with
             fallback_from and cache_usage, cache_manifest.json,
             cache_events.ndjson, metrics prompt_cache_* aggregates,
             running-summary "Provider preflights" section.
Ten minutes: add MINIMAX_API_KEY and run; preflight receipts show ok with
             cache_creation tokens, lane receipts show cache_read tokens on
             subsequent calls. Add OPENCODE and set provider-policy:
             primary-with-fallback; kill the MiniMax key to watch every
             lane fall back at preflight with fallback_from receipts and a
             green gate.
```

## Validation commands

```bash
ub-review doctor --profile gh-runner
                      # provider env present/missing before any spend
cargo test --bin ub-review --locked runtime_fallback_retry
                      # retry guards: transient-only, one per lane
cargo test --bin ub-review --locked model_lane_scheduler_honors_provider_max_concurrency
                      # provider max_concurrency caps scheduled waves
cargo test --bin ub-review --locked provider_preflight_cache_selection
                      # cache mode pinned to minimax+anthropic-messages
python scripts/verify-bun-review-artifacts.py --self-test
                      # secret-name guard incl. FACTORY_API_KEY
jq '.[] | {provider, model, status, cache_usage}' \
  target/ub-review/review/provider-preflight-status.json
jq 'select(.kind != "shared_context_prepared")' \
  target/ub-review/review/cache_events.ndjson
                      # cache hits per lane on a real packet
```

## Implementation PR slices

This spec routes the remaining work:

1. DONE: `[providers].policy` parses into resolved provider policy when the
   CLI/env policy is `auto`; explicit CLI/env policy still wins.
2. DONE: `[providers.minimax].max_concurrency` and
   `[providers.opencode].max_concurrency` cap provider wave slots.
3. DONE: 429/timeout/5xx backpressure sheds the next wave to fallback so a
   rate-limited provider does not keep receiving full waves.
4. DONE: `[providers.minimax].prompt_cache` is executable for
   `explicit-anthropic` and `off`; invalid values are PolicyError receipts.
   Remaining provider config decision: decide whether provider model/env/role
   become executable config or stay descriptive.
5. Candidate slice, no issue yet: fallback specs for the proof-planner and
   follow-up passes, which today hard-pin direct MiniMax and fail without
   it.

## Release note claim

```text
ub-review runs BYOK model lanes on MiniMax M3 with prompt-prefix caching on
by default, optional OpenCode fallback at preflight plus one bounded runtime
retry on transient failures, per-provider wave caps, one-wave backpressure
shedding to fallback, and full provider receipts - terminal provider failures
are recorded as missing evidence and can never redden the gate.
```

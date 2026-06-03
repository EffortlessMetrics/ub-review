# Prompt prefix caching

`ub-review` should treat shared prompt caching as runtime plumbing for wide
MiniMax review lanes. The goal is not to create a mutable shared model session.
The goal is to send the same byte-identical, cacheable prompt prefix to every
lane, then append a small lane-specific task tail.

This is prompt or prefix caching. It is distinct from Anthropic's Contextual
Retrieval pattern: Contextual Retrieval enriches RAG chunks before retrieval,
while prompt caching helps repeated requests reuse a stable prefix that already
fits in the model window. For `ub-review`, the useful pattern is:

```text
one canonical review packet prefix
+ many specialist lane tails
+ provider cache accounting
```

MiniMax M3 is the target provider because the Bun direct-review path already
uses MiniMax M3, MiniMax advertises a long M3 context window suitable for large
review packets, MiniMax documents Anthropic-compatible explicit
`cache_control`, and MiniMax usage receipts expose cache accounting such as
`cache_creation_input_tokens` and `cache_read_input_tokens`. The implementation
must keep correctness independent of cache availability: explicit caching is a
performance mode, automatic caching is a fallback, and no-cache execution is
still a valid review run.

Reference provider docs checked for this design:

- MiniMax M3 model page:
  <https://www.minimaxi.com/models/text/m3>
- MiniMax explicit Anthropic-compatible cache control:
  <https://platform.minimax.io/docs/api-reference/anthropic-api-compatible-cache>
- MiniMax prompt caching overview:
  <https://platform.minimax.io/docs/api-reference/text-prompt-caching>
- Anthropic prompt caching semantics:
  <https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching>

## Design target

The stable shared prefix should contain the expensive, reusable review context:

```text
system / policy
schemas
PR thread context
diff packet
RIGHT-side line map
changed-file context
tokmd/ripr/unsafe-review/ast-grep receipts
proof plan / existing receipts
UB ledger excerpt
run-start observation ledger
```

Each lane call should be shaped as:

```text
cached shared prefix
+
small lane-specific task
```

A 20-lane run should therefore behave structurally like:

```text
1 cache write
20 cache reads
20 small lane tails
```

rather than:

```text
20 full prompt ingests
```

The intended win is orchestration headroom. If a PR packet is 150k tokens, a
20-lane no-cache fanout asks the provider to process roughly 3M repeated input
tokens. With a warm cache, the runner pays one cache write, then lane requests
reuse the prefix and only add small lane prompts. Report only provider-returned
usage; do not invent savings.

## Non-goals and invariants

Prompt caching must not change review semantics.

- It is not a shared conversation or mutable model session.
- It is not a replacement for artifact receipts.
- It is not a reason to post lane status tables in the PR body.
- It must not put provider/cache telemetry into the GitHub PR review body.
- It must not hide missing model evidence or provider failures.
- It must not make correctness depend on a durable external cache.
- It must not include secrets, API keys, tokens, or private provider receipts in
  the cacheable prompt block.

The cacheable block is an immutable run artifact. Any timestamp, random order,
lane name, lane question, proof delta, or post-cache observation inside the
block changes the prefix and can break reuse.

## Cacheable artifact contract

Add stable artifacts under `target/ub-review/review/`:

```text
shared_context_cache_block.md
shared_context_hash
cache_manifest.json
```

`shared_context_cache_block.md` is the exact byte sequence used as the reusable
prompt prefix. It should be built from deterministic inputs in stable order:

1. reviewer contract and safety policy;
2. output schemas;
3. no-boilerplate and no-LGTM rules;
4. PR body and review-thread snapshot when available;
5. diff and RIGHT-side line map;
6. bounded changed-file context;
7. sensor receipts and statuses;
8. proof plan and receipts that already exist before cache warm;
9. UB ledger excerpt;
10. run-start observations.

The block must exclude:

- timestamps and wall-clock data;
- lane ids, lane names, lane-specific questions, and provider names;
- random ordering or nondeterministic map serialization;
- proof receipts produced after the cache was warmed;
- follow-up observations produced after the cache was warmed;
- provider usage receipts and cache metrics;
- anything that belongs only in a lane tail.

`shared_context_hash` should be the SHA-256 hex digest of the exact bytes in
`shared_context_cache_block.md`. `cache_manifest.json` should record how the
block was built without introducing nondeterministic fields into the block
itself.

Example manifest shape:

```json
{
  "schema": "ub-review.prompt-cache-manifest.v1",
  "provider": "minimax",
  "model": "MiniMax-M3",
  "mode": "explicit_anthropic",
  "shared_context_path": "review/shared_context_cache_block.md",
  "shared_context_hash": "sha256:...",
  "shared_context_bytes": 812345,
  "shared_context_tokens": 182000,
  "cache_breakpoint": "system[-1]",
  "canary_status": "passed"
}
```

Do not add timestamps to this manifest unless they are needed for TTL accounting.
TTL fields belong in runtime cache receipts or metrics, not in the hashed prompt
block.

## Prompt layout

Put stable content first and mark only the reusable prefix as cacheable:

```text
[cacheable]
- reviewer contract
- output schema
- no-boilerplate rules
- PR body/thread snapshot
- diff and line map
- sensor receipts
- ledger excerpt
- known observations at run start

[not cacheable]
- lane id
- lane question
- relevant new observation deltas
- proof receipts produced after warm
- prior lane result
- final instruction for this call
```

For MiniMax's Anthropic-compatible endpoint, prefer explicit cache control after
a canary proves that MiniMax M3 accepts it. The cache breakpoint should be at the
end of the static block, for example by representing the static block as a
content block with:

```json
{"cache_control": {"type": "ephemeral"}}
```

Later lane calls must send the same static prefix bytes and change only the
non-cacheable user tail. If the provider requires repeated `cache_control`
markers on later calls, keep the marker position and static bytes identical.

## Provider capability model

Provider resolution should expose prompt-cache capability explicitly:

```rust
prompt_cache: none | automatic | explicit_anthropic
```

MiniMax resolution policy:

```text
default: explicit_anthropic if the canary passes
fallback: automatic
fallback again: none
```

Fallbacks are performance-only. A failed cache canary should not fail the review
unless all model execution fails. Instead, record the effective mode and continue
with automatic or no-cache execution.

The canary should verify all of the following before enabling explicit mode:

- MiniMax M3 accepts the Anthropic-compatible request shape with `cache_control`;
- the primer response returns usage accounting;
- `cache_creation_input_tokens` is positive for the static block;
- a second small request with the same prefix returns positive
  `cache_read_input_tokens` or equivalent provider cache-read usage.

If these checks cannot be proven, use automatic caching if available. If neither
mode can be proven, run without prompt-cache assumptions and still emit all
review artifacts.

## Execution sequence

The scheduler should warm the cache before wide fanout:

```text
1. Build canonical shared context.
2. Hash it and write cache artifacts.
3. Run the explicit-cache canary when MiniMax is selected.
4. Make one cache-primer call.
5. Confirm cache_creation_input_tokens > 0 when explicit mode is active.
6. Fan out primary lanes in parallel.
7. Check cache_read_input_tokens on each lane.
8. Run tests/proof broker in parallel.
9. Inject proof/observation deltas into follow-up calls.
10. Refresh cache if proof work risks exceeding the cache TTL.
11. Compile one concise review.
```

The primer prompt should be cheap and non-reviewing:

```text
Acknowledge the review context is loaded. Do not review yet.
```

Do not start every lane before the primer completes. If all lanes launch first,
they can all become cache-write misses.

## Follow-up calls

Follow-up calls reuse the same cached prefix and include only small deltas in the
uncached tail:

```text
same cached prefix
+
lane prior result
+
new relevant observations only
+
new proof receipts only
+
follow-up question
```

Never rebuild the shared prefix to include post-warm observations or proof
receipts. Rebuilding the prefix creates a different cache key and defeats reuse.
If a later run needs those receipts in the shared block, that is a new run with a
new `shared_context_hash`.

## TTL guard and keepwarm

MiniMax explicit caching is documented with a five-minute lifetime that refreshes
on hits. Runtime code should treat the TTL as provider behavior, not as a durable
state guarantee.

Track these fields in runtime receipts or metrics:

```text
cache_warmed_at
last_cache_hit_at
proof_job_expected_finish
refresh_if_needed
```

If the proof broker is expected to run longer than the cache TTL, issue a small
keepwarm call before follow-up fanout. The keepwarm call should reuse the same
cached prefix and a tiny non-reviewing tail, and it should record whether it got
a cache read.

## Metrics

Add factual cache metrics under `review/metrics.json`:

```json
{
  "prompt_cache": {
    "provider": "minimax",
    "mode": "explicit_anthropic",
    "shared_context_hash": "sha256:...",
    "shared_context_tokens": 182000,
    "cache_creation_input_tokens": 182000,
    "cache_read_input_tokens": 3640000,
    "lane_cache_hits": 20,
    "lane_cache_misses": 0,
    "estimated_input_tokens_avoided": 3458000
  }
}
```

Rules for metrics:

- `cache_creation_input_tokens` and `cache_read_input_tokens` come from provider
  usage fields or remain absent/null.
- `lane_cache_hits` counts lane calls with positive provider cache-read usage.
- `lane_cache_misses` counts lane calls where explicit mode was expected but no
  provider cache-read usage was returned.
- `estimated_input_tokens_avoided` is derived only from observed cache-read
  tokens, never from planned lane width alone.
- Cache metrics stay in artifacts and summaries, not in the PR review body.

## Review body policy

The PR review body remains pure signal:

```text
confirmed findings
verification questions
summary-only concerns
refuted / dropped
residual risk
missing evidence
```

Do not include successful lane tables, provider/model rosters, cache hit ratios,
or token-savings claims in the GitHub review body. Those are operational facts
for `review/metrics.json`, `running-summary.md`, and downloadable artifacts.

## Implementation slices

### PR 1: Canonical context block

Add `shared_context_cache_block.md`, `shared_context_hash`, and
`cache_manifest.json`. Enforce deterministic ordering, bounded size, no
timestamps, no lane-specific text, no transient proof output, and a recorded
hash.

### PR 2: MiniMax cache support

Add provider capability `none | automatic | explicit_anthropic`. Use explicit
Anthropic-compatible cache control for MiniMax only after the canary passes.
Fallback to automatic caching, then no-cache.

### PR 3: Cache primer and fanout

Warm the cache once before primary lane fanout. Keep the primer output small and
non-reviewing. Do not let primary lanes race each other into cache-write misses.

### PR 4: Delta follow-ups

Keep follow-up calls on the same cached prefix and add only relevant deltas,
prior lane output, and the follow-up question in the uncached tail.

### PR 5: TTL guard

Record warm/hit timing, estimate whether proof jobs may exceed the cache window,
and issue a small keepwarm call when needed.

### PR 6: Metrics

Record provider-returned cache usage and cache hit/miss counts in metrics. Do
not report fake savings. Keep all cache telemetry out of the GitHub PR body.

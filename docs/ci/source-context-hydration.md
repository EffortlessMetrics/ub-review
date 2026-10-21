# Bounded source context and recovery

The current `run --dry-run --model-mode off --posting artifact-only` path can
hydrate a trusted local Git change and PR discussion without compiling that
project. It constructs packets; it does not perform semantic model review.
The standalone source-only judgment/service remains proposed in
[#1349](https://github.com/EffortlessMetrics/ub-review/issues/1349) and
[the mode decision packet](../JUDGMENT_MODE_DECISION_1349.md). This repair does
not activate that service or satisfy its hostile-source containment requirements.

| Path | Evidence it produces | Limit |
| --- | --- | --- |
| Dry-run, model off, artifact only | Exact Git diff/input packet, bounded discussion snapshot, planned/skipped sensor and model receipts | No executed project build/test, semantic clean judgment, confirmed delivery, CI sufficiency or merge permission |
| Integrated run with configured sensors and approved proof | Actual command receipts and independently evaluated gate evidence, plus applicable review artifacts | Only the exact executed inputs, successful receipts and configured obligations support claims |
| Future source-only tower | Verified source input and advisory judgment under its accepted contract | No project execution or owned CI result; imported CI retains its original identity and result |

## Current trusted-local recipe

Use an already installed, identified `ub-review` binary. Supply a trusted config
and exact existing Git commits. Keep output outside the source tree. Obtain
discussion through an existing authorized read connector or a locally retained
fixture; a fixture proves replay behavior, not live GitHub access. No new key is
needed for a file seed:

```bash
ub-review run --root /path/to/repo --config /path/to/trusted-config.toml \
  --base <exact-base-commit> --head <exact-head-commit> --out /path/to/new-packet \
  --dry-run --model-mode off --posting artifact-only --no-github-summary \
  --pr-thread-context /path/to/retained-discussion.md \
  --pr-thread-context-max-bytes 8192
```

This is a trusted-local operating recipe. The normal checkout path is not an
untrusted acquisition sandbox. Trusted-base admission deliberately disables
event, file and credentialed PR context; this change preserves that boundary.
Use exact SHA inputs for thread head binding; symbolic labels remain a separate
revision-propagation concern. A file seed is caller-supplied text, not
authenticated GitHub provenance or a complete changed-source manifest.

## GitHub thread acquisition contract

With existing authorized PR-thread access, three read-only endpoints run
serially: the first 30 issue comments, first 30 review summaries, and first 50
inline comments. No pagination, retries, model call, post or project build is
performed by this acquisition helper. Later pages remain omitted even when all
three requests succeed. The rendered snapshot states that scope.

Each curl request has its own 30-second deadline and captures at most
1,048,576 bytes, including its HTTP-status trailer. Absent Content-Length does
not remove the byte bound. Overflow rejects the whole endpoint, closes the
reader, kills and reaps the child. Failed bodies and stderr are not retained;
diagnostics contain endpoint, exit/status or parse/shape/size cause and recovery.
Curl's user config and output buffering are disabled for thread reads, so an
overflow need not wait for the producer's EOF. There is no response spool or detached reader.
Three stalled endpoints can cost approximately 90 seconds plus small startup
overhead; this is not a shared hard run deadline or a host-level cgroup limit.

If writing curl's configuration fails, the shared spawn helper still attempts
kill followed by reap before returning the original setup error. A failed kill
or reap adds cleanup diagnostics without replacing that original I/O cause.
Failed reap remains explicitly completion-unconfirmed, including `ECHILD`;
it never turns failed configuration into a successful request. Successful reap
confirms completion even if the preceding kill request failed. This diagnostic
repair does not add a supervisor or change process-tree or timeout policy.

Bounded-response capture failures use the same reconciliation: preserve the
original capture error, attempt kill then reap, and retain cleanup diagnostics.
A capture-limit failure stays a capture-limit failure when reap also fails;
failed reap remains completion-unconfirmed. Neither error becomes a successful
response or clean evidence.

Successful endpoints survive an independent endpoint's network, timeout, 5xx,
parse, shape or size failure. Original record IDs, full admitted bodies, source
anchors, commit IDs and head bindings remain unchanged. Failed endpoints are
excluded from successful API sources and add incomplete-coverage warnings.
All failed endpoints produce `unavailable`, not an empty successful snapshot.
An admitted array item must be an object. A null or scalar item rejects that
endpoint, including any valid prefix, rather than inventing a source record.
The warning identifies the item ordinal without echoing its value. Empty arrays
remain valid, and omitted items past the first-page limit are not admitted.
Failed HTTP responses stop curl at their headers, so an oversized or stalled
denial body cannot hide its status behind the capture limit. HTTP 401/403 stops
further API reads; resolve access through the existing owner
before retrying. Do not obtain another credential or route around denial.

`thread_context_truncated` covers per-item rendering, excess first-page items
and aggregate text truncation. The configured byte budget applies to rendered
source text, with the existing 13-byte truncation marker in addition. Structured
records deliberately preserve original evidence rather than shortened bodies;
their acquisition is bounded by the three response limits and 30/30/50 item
limits, not by the rendered text budget. The full packet contains other artifacts
and copies of context, so 8 KiB context is not an 8 KiB total-storage guarantee.
The GitHub event reader and general Git/project packet inputs retain their
existing contracts; this is not a blanket memory bound for all CLI inputs.

Retry into a fresh output directory after fixing the named transient failure.
Compare source IDs and original commit/body identities with the retained failed
packet. A recovered run can add the missing endpoint while the earlier failure
receipt remains available. Never relabel historical context as current proof.

## Regenerating packet summaries

`summary --run-dir <packet>` retains the packet's recorded
`missing_or_failed_model_evidence` entries alongside legacy status-derived
issues. An internal-audit write failure recorded for a degraded lane therefore
remains visible in the missing-evidence section. Identical persisted and
reconstructed copies render once;
different reasons remain distinct. Packets without the array retain their
existing status fallback, and contentful degradation alone is not reclassified
as missing evidence. This preserves recorded failures during hydration; it does
not execute providers, repair missing audit files, or add a malformed-packet
recovery contract or a general summary-input memory limit.

## Measured costs and qualification

On 2026-10-13, Linux cloud host: 16 GiB cgroup memory limit, four-CPU quota,
no swap, about 30 GiB workspace free. One serial partial/recovered pair replayed
the connected GitHub snapshot of
[PR #1400](https://github.com/EffortlessMetrics/ub-review/pull/1400) over actual
ub-review source commit `903a87f23d3f2d04f293af512e692659775902d4`. The fixture
retained comment `6114563897` and restored review IDs `5485322566` and
`5485348167`; it injected a local 503, then recovered. This was local transport
replay of connected reads, not live curl/provider/posting acceptance.

Runtime PATH contained only Git and curl: no Cargo or rustc. Each child inherited
256 MiB address-space, 10s CPU and 64 MiB per-file limits. These were campaign
controls, not limits installed by ub-review. `wait4` measured the CLI and its
waited children; max RSS is the largest process high-water value, not an
aggregate concurrent-tree peak. Python HTTP fixture and source checkout size
are outside those child metrics.

| Snapshot | Wall | User + system CPU | wait4 max RSS | Packet logical bytes / files | Context JSON |
| --- | ---: | ---: | ---: | ---: | ---: |
| Partial | 0.121s | 0.118s | 16,584 KiB | 746,829 / 235 | 4,125 bytes |
| Recovered | 0.119s | 0.117s | 16,576 KiB | 777,974 / 235 | 8,353 bytes |

Binary SHA256: `412335c6a971b7da6c2404713cf090db41d301a1bb6d238efeecb66f8d2bf7dc`.
Connected snapshot SHA256:
`ae8f7fe3ee433e7ae75ecff962dba0e912992d3067ee02d576c11766d9c07993`.
Campaign inputs, raw packets, script and costs are retained under
`/workspace/evidence-1407/`; this local path is not a hosted artifact link.

The twelve actual-CLI regressions include a real 30s stalled read, capture overflow
without Content-Length, cutoff while a producer withholds EOF, exact capture
boundary including the status trailer, malformed JSON/shape/items and recovery, UTF-8/zero-budget truncation,
partial-to-recovered records, permission-denial stop and oversized 401/403
denial bodies, plus persisted model-evidence hydration and legacy/healthy controls.
On Linux they observe
live acquisition children through procfs and require their absence after return.
The prior six-test suite took 31.93s at its retained candidate, before adding
the oversized-denial regression. Twelve existing
context tests also passed. Engineering qualification compiles ub-review itself;
that cost and its result are separate from the compiler-free consumer runtime.

No sustained load, multi-lane model campaign, quota/inode exhaustion, hostile
repository or operated host was qualified. There is no measured fifty-lane
capacity claim. Do not scale these two short runs into a capacity estimate.

# Task projection reconciliation

`reconcile-task-projections.py` compares the persisted TaskLedger with existing
queue, portfolio, receipt, lease, Required-proof and accounting projections.
It is a read-only shadow verifier, not a scheduler or a second gate authority.

## Run

From the repository root, using Python 3.10 or later:

```sh
python scripts/test-task-projections.py -v
python scripts/verify-bun-review-artifacts.py target/ub-review
python scripts/reconcile-task-projections.py target/ub-review --write-report
```

The ordinary verifier remains required: the reconciliation command reuses its
revision-binding and TaskLedger event/snapshot replay functions, but does not
repeat every ordinary packet-schema check. Run against a completed, immutable
local packet in an unprivileged environment. This is not an isolation boundary
against a concurrently writing process.

For standalone worker output, select the different receipt layout explicitly:

```sh
python scripts/reconcile-task-projections.py target/worker-proof --kind worker
```

The standalone worker does not publish `input/revision-admission.json`. Worker
reconciliation therefore derives a **shadow comparison binding**, not revision
admission authority, by requiring exact agreement among the revision-stamped
`proof_receipt.json`, the published head `resource_lease.json`, and the
replay-verified TaskLedger event/snapshot pair. The receipt head must equal the
binding's reviewed commit and the lease consumer must equal the proof ID. This
is enough to detect disagreement inside one worker packet; it does not prove the
producer or elevate worker output into merge authority.

An unresolved typed worker proof is coherent only in its exact production
shape: the nightly preflight is a separately receipted ledger task, the head
command is skipped, the aggregate result is `skipped_unresolved`, and the
published head lease is `refused` with zero resources. A skipped head paired
with a granted lease, or an unresolved result paired with an executed head, is
a contradiction. The preflight may fail during setup or be cancelled: its
skipped receipt must join the matching released `SetupFailed` or `Cancelled`
task. A missing preflight task or a claimed execution without that ledger state
is contradictory; unavailable setup is not invented physical execution. This
preflight ownership check applies to every worker result, including passed heads.

For the retained #961 incidents, use `--legacy` with a case directory under
`fixtures/authority-incidents`. Legacy packets do not gain current-revision
or execution authority from this verifier. The regression suite consumes the
manifest's original bytes, digests, sizes and expected violation codes without
adding incident-specific branches to production logic.

## Result

The command prints deterministic JSON. `--write-report` also replaces
`review/task_projection_reconciliation.json` atomically. It invalidates an old
report before starting; an input or publication failure cannot leave an old
successful report looking current. The default invocation writes nothing.

| Exit | Meaning |
| --- | --- |
| `0` | The available compared projections are coherent with the replay-verified ledger. |
| `1` | A contradiction was found, or current ledger/revision verification is unavailable. |
| `2` | Input, output, decoding, shape or resource limits prevented completion. |

**Coherent does not mean PASS.** Correctly recorded deterministic failure or
missing Required evidence can be coherent. Unsatisfied consumers are explicit
observations; a claimed pass without the required receipt is a contradiction.
The existing Rust `gate_outcome` policy is neither replaced nor rewritten.
A report must be recomputed from the packet, not trusted because a JSON file
already exists. Source hashes identify the bytes read; they do not authenticate
their producer.

The contained self-gate enforces the regression suite and runs reconciliation
before artifact upload. The live comparison step is non-blocking because current
production projections still disagree; it retains those disagreements rather
than changing the existing decision or silently repairing historical evidence.
A missing report or a timed-out checker is unavailable evidence, not coherence.
Malformed or unreadable input sets `input_unavailable` and returns exit 2, even
when the report retains useful diagnostics from other readable projections.
`observations_truncated` is separate from `issues_truncated`; many non-blocking
observations do not turn coherent accounting into a contradiction.
Nested collections and objects are validated before iteration. A malformed
route lease list, task consumer/reservation list, queue lease, Required-proof
map, runtime map, calibration count map, or metrics run map cannot suppress
independent diagnostics or leave a stale coherent report in place.
Review, worker and queue lease quantities are checked against their Rust
unsigned domains (32-bit CPU, 64-bit memory, disk and timeout) before reservation
comparison. Missing, negative, fractional, boolean or out-of-range quantities
are unavailable input and retain the exit-2 JSON report.
Command durations and their sum use the Rust 128-bit unsigned domain. Every
duration is validated even when metrics are absent; malformed values and sum
overflow remain unavailable instead of being silently omitted from accounting.

The portfolio's top-level head must name the admitted reviewed commit. A
different object ID is contradictory. The current v1 producer can retain a
symbolic `DiffContext` label such as `HEAD`; that label is unavailable immutable
identity, so it cannot establish a coherent current packet. Legacy packets
without admitted identity retain their explicitly unbound verification tier.

Current review packets require the receipt-route artifact. Every proof receipt
must have exactly one route, and its lease IDs must name every owned lease once,
including refused leases. Ordering is not authority. Standalone worker and
legacy packets may omit this artifact; any published route inventory is checked.

## Compared surfaces and limits

The checker validates receipt ownership, proof side identity, terminal state,
resource release, lease quantities, queue/portfolio inventory, source-request
presence, Required-proof counts and links, sensor fill entries, receipt routes,
and duplicated scheduler/metric values. It distinguishes setup failure from a
spawned process cancellation, and worker preflight from the requested proof.

Some current fields have narrower meanings than their names suggest:

- `calibration.v0.counts.proof_requests_executed` counts proof receipts, including
  skipped receipts. It is not the number of physical commands or source requests.
- Command receipt duration and TaskLedger process span have different timing
  boundaries. Differences are observations, not invented clock equality.
- Similar commands are candidate groups with `equivalence: unproven`. No tasks
  are merged and no avoided execution or financial savings is claimed.
- The standalone worker does not publish a separate preflight lease. Scheduler
  and cost v1 do not expose complete task counts or whole-workflow cost. Coverage
  records those limits instead of filling them with zero.

Inputs are limited to 4 MiB per file, 32 MiB total, 256 files and 16,384 rows per
array. Reads may consume one extra probe byte to establish an exceeded limit;
rejected bytes still consume the aggregate read budget. Issues and observations
are bounded; the serialized report has a 512 KiB hard limit. The reader rejects duplicate JSON keys, non-finite numbers, unsafe
relative paths and symlink components. The report omits raw commands, prompts,
subprocess logs and free-form diagnostic reasons. These reader/report limits do
**not** bound production subprocess output or the complete gate packet; #1269
still owns that work.

## What remains before #957 closes

This slice supplies executable cross-projection checks, generated coherent
model-off/model-on/worker fixtures and retained historical negative cases. It
adds the check to this repository's contained CI packet, not every CLI or worker
publication boundary. The remaining #957 work must establish production-generated
coherent packets, complete missing projection/source relationships and reuse
this evidence before #958-#962 change authority. Until that work is proven,
existing CI checks stay in place and this report cannot certify rollout readiness.

The Python implementation is an explicitly temporary extension of the existing
artifact-verifier boundary. Issue #1292 owns moving the projection reader,
reducer joins, bounded report contract, and parity corpus into Rust before the
current policy review date. Its exit condition includes a retained-packet
parity window and deletion of `scripts/reconcile-task-projections.py` and
`scripts/test-task-projections.py`; policy expiry is not an implicit migration.

# Build-readiness checkpoint

_Audit date: 2026-09-08 UTC. Source: main at
[`5e3a043`](https://github.com/EffortlessMetrics/ub-review/commit/5e3a043f70f33505f62672405e2fae36b196081b).
Documentation reconciliation: [#1297]._

This checkpoint records what builders should recheck before selecting work.
[Issue #945] remains execution-order authority, and [Product state] records
earned capability. This is a dated board assessment, not another roadmap or
a release-readiness declaration.

## What the audit established

The initial full GitHub board contained 263 open issues and three open PRs;
creating [#1297] brought the issue count to 264. Open issues were unassigned,
but assignment absence did not establish availability: PR [#1289] had recent
active work. Recheck issue comments, PR heads, and branch ownership before
claiming a lane. June plans and the old issue ledger retain useful history;
their tallies and proposed sequences are not the current board.

The [#956] TaskLedger observation of configured, impact, model-request, and
follow-up proof plus standalone workers landed through [#1266]. Receipt-content
verification landed in [#1290]. The latter is only part of [#957]: queue,
portfolio, Required satisfaction, and lease projection reconciliation remain
incomplete. TaskLedger still observes execution in shadow; existing brokers,
leases, and budgets own execution. The legacy `gate_outcome.conclusion` remains
enforcement authority. The next serial product obligation is [#957], followed
by #958, #959, #960, and the complete #962 Horizon A packet proof.

Repository protection required `ub-review/gate` at this audit; the independent
baseline remained advisory. That settings snapshot does not promote a stable
coordinator or prove external sole-gate readiness. The audit also found no
basis to promise that a documentation-only PR receives a cheaper hosted route.

## Bounded next work

| Existing lane | Next acceptance boundary | Ownership and scope |
| --- | --- | --- |
| [#957], PR [#1289] | Reconcile projections across every publication boundary and retain production-generated coherent packets plus contradictory-input proof. Receipt-content joins alone do not satisfy it. | Active product front; coordinate with the PR owner. |
| [#1293] | Reconcile release-specific runbooks and publication claims against actual source and pipeline receipts. | Separate active documentation lane; no release is authorized by this checkpoint. |
| [#913] | Retain the remaining model-off smoke against unsafe-review 0.3.8, including receipt hashes, argv, and exits; parser behavior already landed in [#915]. | Recheck the remaining issue scope before rebuilding parser work. |
| [#1292] | Establish Rust verifier parity before replacing the governed Python verifier. | Preparation only until overlapping PR [#1289] is terminal or its owner explicitly hands off the seam. Keep the existing verifier effective; its next review is 2026-10-10. |
| [#1296] | Reproduce and isolate intermittent Windows fixture failures with current binaries. | Proof reliability work; a passing unrelated run does not close it. |
| [#1270] | Retain actual baseline evidence for the model-off comparison. | PR [#1287] is scaffold work, not the measured acceptance receipt. |
| PR [#1265] | Review its retained v0.1.0 Ubuntu 24.04/glibc 2.39 portability proof against [#1071]. | Preserve that historical scope: Ubuntu 22.04/glibc 2.35 and newer-release support remain excluded. |

The [#1288] hosted concurrency proof remains a prerequisite for [#1275]
measurement. Preserve [#1277] result-plane non-interference before enforcement
changes or live model authority; the complete dependencies stay in [Issue #945].

The development-control programme is [#1217]; its canonical static registry
prerequisite [#1216] remains open. There is no current roadmap CLI to invoke;
use the linked issues and existing repo commands until that command is
implemented and verified.

## Local proof entry points

For this documentation/authority-routing seam, inspect the source and live
issue evidence, then use:

```text
cargo fmt --all -- --check
git diff --check
cargo test --test cli_contract --locked
cargo run --locked --package xtask -- policy-check
cargo run --locked --package xtask -- policy-inventory
```

Review changed local Markdown links and reference definitions as well.
`cli_contract` exercises adoption and artifact-contract assertions, including
README/ROADMAP setup guidance. Policy validation checks receipt metadata and
the Bun pin references; it does not establish that all documentation claims
are true. Record command results
against the candidate commit and obtain independent review plus current-head
hosted checks before merge. This checkpoint is not itself a passing receipt.

[Product state]: ../PRODUCT_STATE.md
[Issue #945]: https://github.com/EffortlessMetrics/ub-review/issues/945
[#913]: https://github.com/EffortlessMetrics/ub-review/issues/913
[#915]: https://github.com/EffortlessMetrics/ub-review/pull/915
[#956]: https://github.com/EffortlessMetrics/ub-review/issues/956
[#957]: https://github.com/EffortlessMetrics/ub-review/issues/957
[#1071]: https://github.com/EffortlessMetrics/ub-review/issues/1071
[#1216]: https://github.com/EffortlessMetrics/ub-review/issues/1216
[#1217]: https://github.com/EffortlessMetrics/ub-review/issues/1217
[#1265]: https://github.com/EffortlessMetrics/ub-review/pull/1265
[#1266]: https://github.com/EffortlessMetrics/ub-review/commit/274d62cf3362f3bfcb47034e6c7d6301efcc5912
[#1270]: https://github.com/EffortlessMetrics/ub-review/issues/1270
[#1275]: https://github.com/EffortlessMetrics/ub-review/issues/1275
[#1277]: https://github.com/EffortlessMetrics/ub-review/issues/1277
[#1287]: https://github.com/EffortlessMetrics/ub-review/pull/1287
[#1288]: https://github.com/EffortlessMetrics/ub-review/issues/1288
[#1289]: https://github.com/EffortlessMetrics/ub-review/pull/1289
[#1290]: https://github.com/EffortlessMetrics/ub-review/commit/3bad5d2781967dd602e691a21333d17c38aaf81c
[#1292]: https://github.com/EffortlessMetrics/ub-review/issues/1292
[#1293]: https://github.com/EffortlessMetrics/ub-review/issues/1293
[#1296]: https://github.com/EffortlessMetrics/ub-review/issues/1296
[#1297]: https://github.com/EffortlessMetrics/ub-review/issues/1297

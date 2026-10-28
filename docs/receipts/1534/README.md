# Retained #1534 watchdog consumer evidence

Evidence-only branch; never merge this branch. Production repair is candidate `c0b14631cc53faae179b3c68a0049745452f442c` (tree `89169d41e134737eb9c5ab3fb3fed05355599a8c`) against `ac06293dd0871531c1826e4a3266c8dc537cf4f6`.

[evidence.tar.gz](evidence.tar.gz) contains the complete source patches, bounded fixture inputs/output artifacts, CLI harness, raw local baseline logs, native staged receipts, full RIPR reports, independent source review and runtime metadata. `watchdog-finalization-evidence/manifest.json` gives every member's size and SHA-256.

Archive: 2,353,926 bytes; SHA-256 `312c75d051d4930e520df7e99a36b1e94ce24bf92392913d1a42c8528a80b9df`.

Before repair, appended controls on unchanged production source reproduced three failures among 18 watchdog tests (15 existing passed). After repair, 19 watchdog tests and 23 actual CLI/serialized-artifact/verifier controls passed. Full workspace: 1,283 passed, one ignored; fmt/check/Clippy/doc/policy passed. Meaningful staged native check: zero blocking findings, ast-grep unavailable. Native ready RIPR analysis remains at six unsuppressed exposure gaps, not strict-zero qualification. No hosted or live watchdog proof is claimed by this archive.

Recreate the dirty before-test source by applying `before-tests.patch` to the base (resulting tree `b53e11e40ab2e2aca6857c86c0b8f8a828fef832`); `repair.patch` reconstructs the final repair tree from the same base. Rust 1.95.0, Python 3.12.14, Linux x86_64. Exact commands and timing are retained in `runtime.json`, `validation.json` and the harness source.

## Exact-head continuation evidence

The current production repair is `8fe9305bc50815a699f9a3e8d62074971ee7c045`, tree `54af5a17b3cf914d3471a15a4c408d5b1e6437ab`, against the same base. This follow-up adds a complete literal receipt test and six exact owned analyzer-limit dispositions; production classifier bytes are unchanged from c0b14631.

[continuation-local-evidence.tar.gz](continuation-local-evidence.tar.gz): 4,736,288 bytes; SHA-256 `60941907cc578653772430be129302af8c9038fd1779fc5b303d09014e2e8a24`. Includes 66 finite payload files and a byte/hash manifest: complete patches, all six compiled rejected defects and restored20/20 positive, full pre/post-disposition native raw reports, zero-blocker staged receipts, ast-grep registry lock/setup and matching controls, affected test Clippy, and first full local current-head workspace result (1,284 passed, one ignored). Local ready RIPR: zero unsuppressed, six owned suppressed gaps,13 unknowns retained; dates and thresholds unchanged.

This archive is the finite local-evidence snapshot captured at 2026-10-20 05:03 UTC. Hosted gate39372144346 was still running; independent baseline39372141445 was red at test exit101, while the same exact-head full workspace passed locally. Denied log/artifact reads remain unverified; no diagnosis or green qualification is inferred. Later hosted terminal state belongs to the [native PR](https://github.com/EffortlessMetrics/ub-review/pull/1538) and its run links. Original c0b evidence remains historical in the first archive. Native source reviews5580916865 and5580924586 found no material source/composition defect; review scope and artifact/attempt limitations remain explicit.

## Same-identity conflict repair, 2026-10-28

Production candidate `246779afc7c43d9b5fa3d33798776bb19788557c`, tree `8e49b644c5f217f1f754349e079a831b49f87dbe`, combines the scoped owner repair with main `eac545ee5ead6026a81b2a9163508ad8a7308072`. The original challenge reproduces an order-dependent terminal pass on `8fe9305`; the repaired classifier rejects both conflicting orders and preserves agreeing duplicates and newer valid identities.

[conflict-controls-20261028.tar.gz](conflict-controls-20261028.tar.gz): **7,284,070 bytes**, SHA256 `afec1c40f9bdc697c5fdd40816282c860a140da601b0c633043261a24a4ac451`. The archive retains 490 payload files and their byte/hash manifest, complete reconstructable source patches and Git bundle, supplied challenge, before/after logs, all CLI fixture inputs and receipts, twelve compiled defect controls and restored positives, complete raw RIPR/precommit reports, source reviews, environment and exact command/results records.

Exact-final local proof passes24 watchdog tests,39 actual CLI/receipt/explicit-verifier controls plus forged-pass rejection, full verifier self-test, fmt,policy, staged native checks and ready RIPR. Raw128 findings retain15 individually owned suppressed gaps,0 unsuppressed and36 unknowns. The full local workspace1335 pass/1 ignored and hosted baseline40652339844 belong to predecessor671160a7; they are retained under their actual identity, not relabeled final-head execution. The twelve compiled defect controls predate main composition but bind to the identical watchdog source. Final-head hosted baseline40652690849 and gate40652693457 were running at this finite local snapshot; their terminal evidence remains in the native PR/run records.

No live artifact/attempt-authority or default watchdog-wiring claim is made. All separate access, prerequisite and integration/activation holds remain with their owners. No #1281/kill/reap repair, denied payload retry or release/deployment transition. This evidence branch remains unmergeable by purpose.

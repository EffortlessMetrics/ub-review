# Independent exact-revision review: bounded native packet reader (#1292 component)

## Decision and subject

No material blocking finding within the documented immutable-packet, reader-only shadow contract. This is local technical clearance for the bounded reader component, not #1292 completion, a full reconciler/gate transition, hosted-check qualification or merge authorization.

- Base/parent: `3dbcc2be9768b082c0896dc2360e950ffbd56a65`
- Frozen local head: `f7c54ca8beb1c36b0267d5376e3be347b3ee945b`
- Frozen tree: `cc08312e4c857994a153a537082f6776b0fc54a7`
- Reviewed all nine changed paths: Cargo.lock; docs/TASK_PROJECTION_RECONCILIATION.md; policy/allow.toml; src/task_projection_reconciliation/reader.rs; src/task_projection_reconciliation/reader/tests.rs; xtask/Cargo.toml; xtask/src/main.rs; xtask/src/task_projection_reader.rs; xtask/tests/task_projection_reader_parity.rs.

The independent detached lab is `/workspace/scratch/2e70149a8e59/review-projection-frozen-f7c54ca8`. Its HEAD and tree were checked before execution and after all experiments. The author checkout, index, branch and receipts were never modified. All experiments were local; no GitHub writes or cloud sessions were created. AGENTS.md, CLAUDE.md, native review-change and AGENTIC_WORKFLOW guidance were read.

## Independent positive execution

Toolchain came from `toolchains/env.sh`: rustc/cargo 1.95.0. Used two build jobs and isolated `review-projection-frozen-target`.

- `cargo test -p xtask --locked task_projection_reader`: all 29 production-reader unit tests passed.
- `cargo test -p xtask --locked --test task_projection_reader_parity`: all 10 executable reader/parity tests passed.
- `cargo test -p xtask --locked`: all 71 unit tests, 1 smoke CLI test and 10 reader/parity CLI tests passed.
- `cargo clippy -p xtask --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo run --locked -p xtask -- policy-check`: passed, 6 policy files / 15 allow receipts / 5 CI lanes / 2 risk packs. Only existing overdue-review warnings occurred; the new dated process-invocation receipt was accepted.
- `git diff --check <base> HEAD` and restored-worktree `git diff --check`: passed.
- `cargo tree --workspace --locked -e features -i serde_json`: only default/std, including both workspace packages; no arbitrary_precision/raw_value feature expansion.

These exercise canonical relative spelling, existing/broken intermediate and leaf symlinks, fixed resolved root spelling, immutable cached success/missing/failure outcomes, exact per-file/aggregate/unique-attempt boundaries, rows/strings/mappings, decoded duplicate keys, UTF-8/BOM/surrogate grammar, maximum container/node/depth boundaries, exact u32/u64/u128 domains, stable diagnostics, source hashing, escaped-inventory report overflow and explicit shadow/non-reconciliation output.

## Compiled production-path negative controls

Each reviewer-only mutant was built, ran the focused production reader or actual xtask CLI path, and produced a test assertion failure with exit 101. Mutants were never published. The final pristine candidate was restored, then all 29 reader unit and 10 CLI positives passed again. Full commands, patches, exact mutated trees and outcomes are retained in `review-projection-frozen-controls/receipts.json` and individual `.patch` / `.log` files.

1. Disable decoded duplicate-key guards in both the preliminary validator and native decoder. Actual valid-schema CLI grammar test rejected the mutant when `{\"x\":1,\"\\u0078\":2}` was incorrectly admitted. Mutant tree `dbb167f7bb7c834e3a9bc8e277cf90188b06ea5f`.
2. Narrow integer_u128 to parsing u64. Actual worker CLI test rejected the mutant because the valid `18446744073709551616` duration became unavailable. Tree `75dd0a7827db2a7f5d46a125cae006adc4eae985`.
3. Disable symlink rejection. Actual sensor-parent-symlink CLI test rejected the success/escape. Tree `c1689b99ffa016c07c01c5e2897ae686284bf3fc`.
4. Remove the read probe byte. Production PacketReader overflow test rejected the mutant because the oversized file was silently accepted. Tree `a7a12b1dcf1d35498c9f424e06f42d96060d73f3`.
5. Bypass successful raw-cache reuse. Production PacketReader cache test rejected the mutant because a revisit returned changed on-disk bytes. Tree `3f817fb0ad45fac5abd93949910210c00451a0f0`.
6. Replace bounded report-overflow receipt with exit 1 / empty stdout. Actual escaped-inventory CLI test rejected the mutant at JSON parsing. Tree `fb383f7f6d4abcb4e4217cb634afd4b229bc34c0`.

One narrow test-oracle limitation is retained honestly: `malformed_source_retains_independent_later_input_evidence` alone did not kill the decoded-duplicate mutant because its fixture also uses an unsupported schema. The independent valid-schema grammar test above did kill it, and the reader unit duplicate controls pass; this is not a suite-level coverage gap or blocker. Evidence: `decoded_duplicate_schema_masked.log`.

## Observed custody limitation, outside claimed isolation

A compiled additional test used the unchanged production PacketReader to construct a reader at an ordinary directory, rename that resolved directory, replace its original pathname with a symlink to an outside directory, and then read an uncached `input`. Rust read the outside bytes as a successful source. The unchanged Python Packet did the same in the corresponding scenario. The test-only tree was `19fede156f70e736f170da0c3a94dea8f8115210`; the reader source itself was unchanged.

Relevant code is reader.rs:189-193 (canonical root pathname) and 277-329 (checks below that pathname, subsequent metadata/open). Docs/TASK_PROJECTION_RECONCILIATION.md:19-21 already requires completed immutable local packets in an unprivileged environment and expressly disclaims isolation against a concurrent writer. Therefore this is an inherited, documented custody limit, not a new blocker or a race-proof containment claim. Any future requirement to process concurrently mutable/untrusted directory trees must separately address root/ancestor custody and check/open races; simple pathname checks alone cannot prove race-free confinement.

Evidence: `review-projection-frozen-controls/resolved_root_custody.patch`, `resolved_root_custody.log`, `custody-summary.log`. Pristine source restored afterward and the 29 reader unit tests reran successfully.

## Remaining proof boundaries

- No full Rust reducer joins, revision/event replay, projection reconciliation, lifecycle/report publication, complete Python corpus parity or ten-retained-packet parity window is established. They are explicitly still open in the new docs.
- This review did not independently rerun the entire root binary regression/artifact-verifier/Python baseline; the parent separately reports the unchanged author's full baseline passed. Our independent proof is the scoped execution above.
- No formal hosted PR-review state, required hosted checks, Ready or merge qualification was established in this local-only assignment.
- Existing Python scripts and CI remain operative; the native output explicitly reports `authority: reader-only-shadow` and `reconciliation: not_performed`, with no full coherent/gate claim.

## Retained logs

Under `/workspace/scratch/2e70149a8e59/`: `review-projection-frozen-positive.log`, `review-projection-frozen-all-xtask.log`, `review-projection-frozen-clippy.log`, `review-projection-frozen-fmt.log`, `review-projection-frozen-policy.log`, `review-projection-frozen-workspace-features.log`, and `review-projection-frozen-controls/` including receipts and restored positive logs.

## Published-head identity bridge (2026-10-14 20:22 UTC)

Independently inspected the fetched local commit object `e5fc48da60bc1b887cfdc4efc2ee28d942a0a279`, identified by the parent as published on `rook/1292-native-projection-reader`.

- Published head: `e5fc48da60bc1b887cfdc4efc2ee28d942a0a279`
- Its sole parent/base: `3dbcc2be9768b082c0896dc2360e950ffbd56a65`
- Its tree: `cc08312e4c857994a153a537082f6776b0fc54a7`
- Frozen reviewed head: `f7c54ca8beb1c36b0267d5376e3be347b3ee945b`, with the identical sole parent and tree.

`git cat-file -t`, `git show -s --format='%P%n%T'`, direct tree equality checks, and `git diff --exit-code <frozen> <published>` all confirm this metadata-only bridge. The nine changed paths against the common parent are unchanged. The review lab remains pristine at its frozen head/tree.

Therefore the source review, positive checks and compiled-control evidence above apply without source reruns to the exact published head `e5fc48da60bc1b887cfdc4efc2ee28d942a0a279`. No material blocker is identified within the same immutable reader-only component scope. This bridge proves local object/base/source identity, not current remote-ref state or hosted event/check qualification. Draft PR preparation is compatible with this bounded result; Ready and merge remain outside this receipt.

Compact identity evidence: `review-projection-frozen-controls/published-identity.txt` and `published-changed-paths.txt`.

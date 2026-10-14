# Frozen RIPR development-source comparison for UB-review #1461

At UB-review head **e5fc48da60bc1b887cfdc4efc2ee28d942a0a279**, the compared RIPR development executable reports **437 unsuppressed gaps / 855 findings / 376 unknowns**. Its complete typed JSON and a separately executed native badge agree. Eligible reader and command production remains; both separate test-only sources have zero current subjects.

The released hosted gate remains **FAIL: 313 / maximum 0**. These development-source findings do not qualify an immutable release, establish consumer eligibility, clear adoption, or authorize a pin, gate, suppression, threshold or policy change. CLI exit 0 establishes successful analysis, not that the consumer gate passed.

## Exact identities

- Compared producer: [ripr-swarm 89ac47a603092ba7f9f55cf74999b88649621abf](https://github.com/EffortlessMetrics/ripr-swarm/commit/89ac47a603092ba7f9f55cf74999b88649621abf), tree `412e1306aa80229cebe20a6fd8e1857162fcf933`. Rust/Cargo 1.95.0, Linux x86_64, default features, development/debug build. Locked `cargo check -p ripr --bin ripr --jobs 2` and `cargo build -p ripr --bin ripr --jobs 2` passed.
- Compared executable: `ripr 0.11.0 (89ac47a603092ba7f9f55cf74999b88649621abf)`, SHA256 `e9f26175806fd8a7936e37083e7fc5c04341c1cb491cee15ef58ac6e19cc4a13`.
- Latest upstream main observed at **2026-10-14 21:00:35 UTC**: [30a505bb67e598768b36401aa88095cb7a79c68c](https://github.com/EffortlessMetrics/ripr-swarm/commit/30a505bb67e598768b36401aa88095cb7a79c68c). That later head was not rebuilt or compared; no finding or fix qualification transfers to it.
- Frozen consumer: [UB-review #1461](https://github.com/EffortlessMetrics/ub-review/pull/1461), head [e5fc48da60bc1b887cfdc4efc2ee28d942a0a279](https://github.com/EffortlessMetrics/ub-review/commit/e5fc48da60bc1b887cfdc4efc2ee28d942a0a279), tree `cc08312e4c857994a153a537082f6776b0fc54a7`, base `3dbcc2be9768b082c0896dc2360e950ffbd56a65`.
- Compared patch: 94,214 bytes, SHA256 `4e541b2d25ee596a5c5f199babb983c028bcb07f416648242cc37a9101f183f0`. Overall diff: 9 files / 2,550 added+removed lines. Rust diff: 5 files / 2,495 lines.
- No `ripr.toml` was discovered. Built-in defaults and production-like-target opt-in state were unchanged. Suppression SHA256 `707a4764d449d7f8201a17ec2ec835fc959be36eec83de9264997e1430cba58e`; UB config SHA256 `00ea80e9ac5ebfb0fb0d43778a423e0447b2cc5a2c2f70e6ca541e876f9698aa`. Tracked source and consumer trees stayed unchanged.

See [receipt.json](receipt.json) for toolchain, build, schema and configuration identities.

## Default failure and supported comparison recovery

Every invocation used an isolated exact-head consumer checkout, an absolute pinned executable, `check --root . --diff target/xtask/ripr/diff.patch --mode ready --format json` or `badge-json`, and `RAYON_NUM_THREADS=2`.

1. **Preserved default preflight failure:** [default-failure.json](default-failure.json) and [stderr](default-failure.stderr). Exit 2, `diff_scope_oversized`, explicit `downstream_consumable=false`. The 2,495 changed Rust lines exceed the unchanged 2,000-line hard limit. Selected: 0 files / 0 lines. Unanalysed: all 5 Rust files / 2,495 lines. Findings cap was not reached. No typed `analysis_outcome` is emitted. The empty findings array is not a complete zero or a consumable gate result.
2. **Supported process-local recovery:** added only `RIPR_PARTIAL_DIFF_LINE_BUDGET=2495`, `RIPR_MAX_DIFF_CHANGED_RUST_LINES=2495`, and `RIPR_CHECK_FINDINGS_BYTES=67108864`. Primary JSON exit 0; `analysis_complete=true`, `complete_with_findings`. All 5 Rust files / 2,495 lines were selected; 0 files / 0 lines omitted. Typed counts: 9 overall files / 2,550 lines, 751 candidate lines, 855 probes, 855 findings. All 855 findings were rendered; 64 MiB cap inactive; no partial-scope or run limitations. Exact producer check-schema validation returned zero errors.
3. **Separately completed native badge:** exit 0, schema 0.8, status `warn`, message `437`; 437 unsuppressed gaps, 855 findings, 376 unknowns, 310 analyzed-test identities, zero suppressed gaps. Its typed outcome, input digest and counts agree with primary JSON. [current-badge-projection.json](current-badge-projection.json) preserves these fields.

Unchanged limits: 200 partial-diff files, 10,000 indexed files, eight rendered related tests per finding, 256 MiB CLI input. Default partial-diff line budget was 1,000 and default findings-array byte cap was 1,000,000. Recovery limits are comparison resources, not a consumer gate/policy change. Full-scope output has no explicit gate-eligibility field; consumer eligibility is unestablished.

The completed current invocations retain `tests/common/cli_lock.rs:rust_module_ambiguous_parent` on stderr. That indexed-context caveat remains visible alongside the complete typed diff outcome. Related-test internal totals are disclosed, but identities beyond the rendered eight are unverified. The raw badge retains 5,700 historical-suppression warnings; they are omitted only from the compact badge projection.

JSON ran 20:30:14–20:43:05 UTC; badge ran 20:43:05–20:55:46 UTC. Debug/source versus registry-release profiles confound performance comparison. Completion was observed; direct CPU advancement/work-total evidence was not available. An optional repeat was cancelled at 20:58:08 UTC after both outputs completed, with zero-byte stdout and driver exit 130. [Cancellation receipt](cancelled-optional-repeat.json). **No repeat-determinism claim is made.**

## Released baseline and hosted correspondence

The released local executable is RIPR 0.10.0, SHA256 `b14f844c990b55e5b3d9f7daa43085cf016824fc8f6f55753efa8159dc5fe432`. Its schema-0.6 badge is byte-identical to the hosted artifact's named `sensors/ripr/gate-decision.json`: **313 unsuppressed gaps / 1,689 findings / 668 unknowns / 1,258 analyzed-test identities**. [Released badge projection](released-badge-projection.json).

[Hosted run 38631520667](https://github.com/EffortlessMetrics/ub-review/actions/runs/38631520667), job 116084066974, artifact 11926772048: ZIP SHA256 `7150ad46fff968009f79a9637ae0697cac06fd92904bd692ecd8da0b6b7283e2`. Named `input/diff.patch` is byte-identical to the compared patch. Named released stdout is 501,493,837 bytes, SHA256 `cf1b689e636a2f4204c3b0eae2db8e47a394c55b29d9a5fe0ee098e9c9ecdbaf`; all 1,689 findings were streamed into the retained released projection and match its envelope count. No large raw stdout is bundled.

The fetched [merge commit 91542b37859461fad395cd7b8b2dbc7c329109e1](https://github.com/EffortlessMetrics/ub-review/commit/91542b37859461fad395cd7b8b2dbc7c329109e1) has the compared head tree and parents equal to the frozen base and head. Hosted admission nevertheless records `worktree_dirty=true`. Tree equality and exact patch/badge bytes preserve, rather than erase, that caveat. Matching badge bytes do not independently attest the hosted executable digest.

Hosted `review/gate_outcome.json` records the sole configured blocking reason: `new_unsuppressed=313 exceeds configured maximum 0`. Artifact verification passed separately. Required proof 0/3 and usable model lanes 0/13 remain distinct unproven evidence. [hosted-byte-correspondence.json](hosted-byte-correspondence.json) records named-member hashes, merge identity and gate projections.

## Actual target and complete source-owner census

[xtask reader lines 11–12](https://github.com/EffortlessMetrics/ub-review/blob/e5fc48da60bc1b887cfdc4efc2ee28d942a0a279/xtask/src/task_projection_reader.rs#L11-L12) explicitly import the physically `src/`-located reader. [Reader lines 948–950](https://github.com/EffortlessMetrics/ub-review/blob/e5fc48da60bc1b887cfdc4efc2ee28d942a0a279/src/task_projection_reconciliation/reader.rs#L948-L950) include its tests under `cfg(test)`. The main UB binary does not register this reader module. All current subjects therefore belong to the **xtask executable**, not the main UB binary. The xtask integration/CLI tests are a separate evidence target.

| Source | Current findings | Current gaps | Released test-only subjects / gaps |
| --- | ---: | ---: | ---: |
| `src/task_projection_reconciliation/reader.rs` | 618 | 316 | — |
| `xtask/src/main.rs` | 7 | 5 | — |
| `xtask/src/task_projection_reader.rs` | 230 | 116 | — |
| `src/task_projection_reconciliation/reader/tests.rs` | 0 | 0 | 31 / 7 |
| `xtask/tests/task_projection_reader_parity.rs` | 0 | 0 | 111 / 44 |

Classes: 42 exposed, 433 weakly_exposed, 4 reachable_unrevealed, 79 infection_unknown, 37 propagation_unknown, 260 static_unknown, 0 no_static_path. Unknowns remain separately counted; default gap policy excludes them.

[current-findings.tsv](current-findings.tsv) contains **every one of the 855 current IDs**, source path, line, class and source owner with function span and Cargo target. Owners were derived from innermost containing Rust AST function spans using Rust 1.95 and already-built `ra_ap_syntax` 0.0.349. Outside-function runtime configuration literals remain eligible production.

Exact-ID correspondence: 305 common, 1,384 release-only, 550 current-only. Of the released 313 gap IDs, 61 remain gap-class, 5 become non-gap and 247 have no exact current counterpart. Source-coordinate grouping is separate: 708 common, 981 release-only, 139 current-only. ID absence/addition and coordinate grouping do not prove fixed/new semantics; seeding and expression spelling changed. [Correspondence summary](correspondence-summary.json).

## Representative owners and bounded next steps

[owner-examples.json](owner-examples.json) preserves five full representative rows, including all rendered related-test evidence and exact source owners.

- **Incorrect target/receiver attribution, [ripr-swarm #7415](https://github.com/EffortlessMetrics/ripr-swarm/issues/7415):** private xtask `JsonValue::as_object`, reader line 46, probe `match_arm:0056a34c`, incorrectly credits main test `tool_policy_known_keys_match_serialized_fields` as `direct_owner_call` / high. [That test](https://github.com/EffortlessMetrics/ub-review/blob/e5fc48da60bc1b887cfdc4efc2ee28d942a0a279/src/main.rs#L15624-L15646) serializes `ToolPolicy` with `serde_json::to_value`; its receiver is `serde_json::Value`. Of 437 gap rows, 216 have positively rendered foreign-target relations, including 53 with foreign `direct_owner_call` / high. The other 221 have no foreign relation in the rendered eight only. Valid indirect CLI evidence must be preserved; this concrete incorrect direct-call identity does not invalidate all cross-target evidence or weak gaps.
- **Private-callee observer candidates, [ripr-swarm #6975](https://github.com/EffortlessMetrics/ripr-swarm/issues/6975) / [#3161](https://github.com/EffortlessMetrics/ripr-swarm/issues/3161):** reader line 297, `error_path:aaf91788` / `SymlinkInput`, remains reachable_unrevealed despite the same-target [exact symlink assertion](https://github.com/EffortlessMetrics/ub-review/blob/e5fc48da60bc1b887cfdc4efc2ee28d942a0a279/src/task_projection_reconciliation/reader/tests.rs#L208-L227). Nested canonical `write`, line 875, `side_effect:321ed19b`, similarly has [UTF-8/u128 exact-byte candidate assertions](https://github.com/EffortlessMetrics/ub-review/blob/e5fc48da60bc1b887cfdc4efc2ee28d942a0a279/src/task_projection_reconciliation/reader/tests.rs#L351-L361). Actual private-callee observer-path adjudication remains necessary; no blanket assertion-helper repair is established.
- **Consumer-owned InputIo adjudication, [UB-review #1461](https://github.com/EffortlessMetrics/ub-review/pull/1461):** reader line 302, `error_path:ecc52b80`, is a distinct `PacketError::InputIo` result. No literal InputIo assertion was located in the two bounded separate test sources. Reconcile independent native proof and the separate pending consumer test repair before classifying it as an analyzer defect. This comparison remains frozen to e5fc48da.
- **Eligible runtime declaration:** xtask reader line 77, `side_effect:3a7b1b89`, is an outside-function `REVIEW_INPUTS` tuple. Declaration placement alone does not make it a test harness.
- **Measured source-role boundary, [ripr-swarm #3213](https://github.com/EffortlessMetrics/ripr-swarm/issues/3213):** both test-only sources have no current subjects, while eligible production remains. UB-review [#1460](https://github.com/EffortlessMetrics/ub-review/pull/1460)'s zero-subject result must not be generalized to #1461.
- **Remaining qualification:** this comparison does not establish fallible-helper admission under [ripr-swarm #7818](https://github.com/EffortlessMetrics/ripr-swarm/issues/7818) / [#7837](https://github.com/EffortlessMetrics/ripr-swarm/pull/7837). Production was measured only at root dot; [ripr-swarm #7617](https://github.com/EffortlessMetrics/ripr-swarm/issues/7617)'s root-coordinate qualification remains unresolved.

The [independent native evidence](https://github.com/EffortlessMetrics/ub-review/tree/bc92afafbd615664a562331e08833a49c8e5d86e/evidence/1461) is separate; this CLI comparison did not rerun native tests or controls. Current badge schema 0.8 differs from the frozen local xtask requirement **RIPR 0.10.0 / schema 0.6**. Hosted counts-only admission remains under [UB-review #1333](https://github.com/EffortlessMetrics/ub-review/issues/1333) / [#1347](https://github.com/EffortlessMetrics/ub-review/issues/1347). Immutable artifact/version/schema/install/adoption proof remains on [UB-review #813](https://github.com/EffortlessMetrics/ub-review/issues/813) → [#1066](https://github.com/EffortlessMetrics/ub-review/issues/1066) → [#1067](https://github.com/EffortlessMetrics/ub-review/issues/1067) → [#1068](https://github.com/EffortlessMetrics/ub-review/issues/1068).

## Bundle contents and integrity

This bundle includes the unchanged default failure, compact typed receipt, abbreviated badge projections, hosted byte correspondence, five full representative rows, correspondence summary, cancellation record and complete 855-row source-owner census. It intentionally omits multi-megabyte raw outputs and working projections. [retained-artifacts.json](retained-artifacts.json) identifies those locally retained artifacts by filename, byte size and SHA256; it does not claim they are public downloads.

[manifest.json](manifest.json) lists bundled file sizes and hashes. [SHA256SUMS](SHA256SUMS) covers all other bundled files, including the manifest; verify from this directory with `sha256sum -c SHA256SUMS`.

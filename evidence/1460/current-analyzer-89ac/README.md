# UB-review #1460: frozen development-source comparison

## Result and boundary

The exact #1460 input has **zero current production-subject counterparts for all 129 released gap IDs**. Current producer source excludes the integration target as `TestEvidence`; it does not promote those rows to exposed, establish their oracles, or prove native tests adequate. No source repair is needed to remove these default harness subjects on the measured development revision.

This is a direct CLI development comparison. It is neither released-artifact acceptance nor consumer gate clearance. The consumer remains pinned to RIPR 0.10.0. The isolated replay of that installed release exactly reproduces the historical badge: **129 unsuppressed / 1,569 findings / 738 unknowns**, SHA256 `14fdb8acd3e3e299bcbb296549158e876764a17a8932f1423491314a34adf727`.

## Frozen objects and tools

- Producer source: `89ac47a603092ba7f9f55cf74999b88649621abf`, tree `412e1306aa80229cebe20a6fd8e1857162fcf933`. Initial and final public main reads matched that SHA. Earlier `7922ca9a` is historical source, not this executable's source.
- Development binary: `ripr 0.11.0 (89ac47a603092ba7f9f55cf74999b88649621abf)`; SHA256 `e9f26175806fd8a7936e37083e7fc5c04341c1cb491cee15ef58ac6e19cc4a13`.
- Linux x86_64, pinned Rust/Cargo 1.95.0, default Cargo features, dev profile. `cargo check --locked -p ripr --bin ripr --jobs 2` and `cargo build --locked -p ripr --bin ripr --jobs 2` both exit 0. Separate owned target; no global install or publication.
- Consumer head `fdb5717688eb266b7b7b427b64104bea6a8ff722`, tree `4ab0d4eee0ac1f9d37181e663a5dfddb7682cf70`, base `3dbcc2be9768b082c0896dc2360e950ffbd56a65`.
- Retained diff: 132,398 bytes; SHA256 `d690685edc7001bc2bfd204b81790d172197ad9331bbd3c6f577613156f95d9e`. Consumer source blob `tests/ci_efficiency_baseline.rs`: `13a1d07a8180773f9122e03179d88857f41863ef`.
- No discovered `ripr.toml`; unchanged built-in defaults and empty production-like-target opt-in. Suppression input SHA256 `707a4764d449d7f8201a17ec2ec835fc959be36eec83de9264997e1430cba58e`: schema 1, 5,700 entries, none matching the historical 129. UB config SHA256 `00ea80e9ac5ebfb0fb0d43778a423e0447b2cc5a2c2f70e6ca541e876f9698aa`.

## Default invocation and supported recovery

All runs use an absolute pinned executable, isolated exact consumer cwd, `check --root . --diff target/xtask/ripr/diff.patch --mode ready --format json` or `badge-json`, and `RAYON_NUM_THREADS=2`. Absolute-root paired invocations change only root spelling.

Default limits are 200 selected changed files, 1,000 selected changed lines, 2,000 maximum changed Rust lines, 10,000 indexed Rust files, 1,000,000 rendered findings-array bytes, 8 rendered related tests per finding, and 256MiB user-input bytes.

1. **Default result:** exit 0; one changed Rust file, zero probes/findings, but `analysis_complete=false`, `partial_with_limitations`, `limited_partial_scope`, `downstream_consumable=false`, `gate_eligibility=ineligible`. The first selected file has 1,832 Rust changed lines and exceeds the 1,000-line budget. It is selected whole; omitted Rust-file/line lower bounds are both 0. The typed overall diff counts are 5 changed files / 2,040 changed lines. Default badge is `warn`, message `analysis-incomplete: partial_with_limitations`; zero is not a clean result. JSON SHA256 `4b3d7997f3f79f93479ef09e2c34f5b81cf99f24553fa8f6693631533d8feb35`.
2. **Supported comparison recovery:** only process-local `RIPR_PARTIAL_DIFF_LINE_BUDGET=1832` changes. File/input/config/pin/threshold/suppression bytes remain unchanged. The actual envelope now confirms `analysis_complete=true`, `no_behavioral_candidates`, 5 total changed files / 2,040 changed lines, 0 candidate lines / probes / findings. The sole Rust file and all 1,832 Rust changed lines are selected; no Rust input is omitted and the findings cap is inactive. Full-scope output has no partial-scope gate-ineligibility field; this does not establish consumer admission. Static badge is `pass`, message `0`. JSON SHA256 `ebc30da1e187f3434795de5f219a979b516ed22fdf2e090bcf0c17bd38a360a2`; badge SHA256 `1b3c02ba49801793f88343b84c3eb83ad6889538fd76ec7009e0abb032912dc9`.

Repeated JSON is byte-identical for both default and recovered dot-root invocations. Dot and absolute recovered badges are byte-identical; their JSON differs in root-bearing fields and has the same inventory, completeness, and counts. All six retained current JSON reports validate against the exact producer's `schemas/ripr/check.schema.json` (schema 0.2). Current native badge schema is 0.8; released baseline is 0.6.

The recovered JSON result is therefore a genuine full-input source-role comparison, while the preserved default result records a distinct incomplete invocation. Increasing a comparison resource limit is not a consumer gate/policy migration.

## Why rows disappear and controls

The exact producer's `analysis/workspace/source_role.rs` classifies `tests/**` as `TestEvidence`; its `seeds_diff_probes` authority excludes that file absent the explicit production-like opt-in. Existing whole-target evidence parsing remains separate from production seam seeding. All 129 old rows lie in that target. `1460-row-correspondence.json` retains each ID, source line, expression, historical classification/bucket, and absent-current-subject disposition.

Existing isolated production-owner control `assertion_shaped_control_production_owner` yields **1 predicate / infection_unknown** with complete analysis; existing `assertion_shaped_oracle_cfg_test` yields **0 probes/findings**, complete/no_behavioral_candidates. These nonempty/zero controls challenge a broken analyzer returning zero globally; they do not prove all test-evidence indexing or native behavior. No native consumer test or mutation campaign was repeated.

Every consumer run separately reports indexed-context `tests/common/cli_lock.rs:rust_module_ambiguous_parent` on stderr. Keep this limitation visible even though it does not create a subject in the recovered typed diff outcome. Badge warnings retain 5,700 unmatched historical suppressions; none cause the zero result.

## Existing owners and next work

- [Source-role #3213](https://github.com/EffortlessMetrics/ripr-swarm/issues/3213): reuse this exact case as supporting acceptance evidence. The 7 declaration, 65 same-target-oracle, and 57 foreign-attribution historical gap buckets have no default current production subject here. Do not start a competing source-role/oracle implementation to repair these old rows.
- [Foreign-target #7415](https://github.com/EffortlessMetrics/ripr-swarm/issues/7415): remains open; this zero-subject input does not exercise or establish repair of target-boundary attribution for eligible production-like subjects.
- [Fallible helper #7818](https://github.com/EffortlessMetrics/ripr-swarm/issues/7818) / [witness PR #7837](https://github.com/EffortlessMetrics/ripr-swarm/pull/7837): remain separate. The PR preserves limitations rather than repairing helper assertion-context admission. No current production counterpart here is evidence of that fix.
- [Relative-member-root #7617](https://github.com/EffortlessMetrics/ripr-swarm/issues/7617): current source still derives ownerless `struct_package_prefix` from the original probe path, while package-prefix handling now rejects absolute forms. This case has no current subject, so root equality here does not qualify #7617 or prove the old path-coordinate defect fixed for all eligible inputs.
- [UB compatibility #813](https://github.com/EffortlessMetrics/ub-review/issues/813) → [released-bundle selection #1066](https://github.com/EffortlessMetrics/ub-review/issues/1066) → [migration #1067](https://github.com/EffortlessMetrics/ub-review/issues/1067) → [installed proof #1068](https://github.com/EffortlessMetrics/ub-review/issues/1068): existing artifact/adoption route. Frozen consumer `xtask/src/main.rs` explicitly expects version 0.10.0 and badge schema 0.6, so direct development execution bypasses the pinned local adapter and establishes no compatibility or required-gate result.

Preserve the genuine overlap-removal native control already reviewed in #1460. The remaining consumer transition needs an accepted immutable public producer and compatibility/install/gate evidence under existing owners, rather than test rewrites, suppressions, threshold changes, a development pin, or a new duplicate issue.

## Retained evidence

`1460-receipt.json` carries exact source/config/schema/tool/output identities and raw envelopes. `1460-row-correspondence.json` carries all 129 historical rows. `1460-schema-validation.json` records schema validation. This published bundle retains the six default/recovered JSON reports, two controls, schema validation, build/check logs, and structured identities. Raw badge digests and exit outcomes are in the receipt; the original released badge remains in the linked hosted #1460 run artifact. Source and consumer tracked trees were unchanged during comparison.


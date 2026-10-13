# Test-only currentness compatibility proposal for #1067

Owner: existing #1067 migration owner; supporting tests: ub-review/tests.
Decision: comparison_only_no_promotion. Required readiness: false.
Historical proposal base: `2fae0dc21c2e178cb8175101dacd666b7b8ccdeb`, tree
`2b18b72daa5a1f9c8c2ae98a227c83aaa964b88d`. The test-only companion is
based on `2b8e66ebe5f11608fb9e85da4af7c5747252f86f`; the affected
production seams are unchanged between those revisions.
No selected generation is supplied by this patch.

## Contract and evidence boundary

The [current owner handoff](https://github.com/EffortlessMetrics/ub-review/issues/1067#issuecomment-6136369952)
reports 46 candidate raw rows, 33 raw unknowns, 30 native current unknowns,
two base_deleted unknowns and one moved_or_renamed unknown. It also retains
26 Python controls and four killed variants. This proposal adds Rust tests at
the repository seams; it does not replace or modify that comparison verifier.

[Source a734bc1](https://github.com/EffortlessMetrics/ripr-swarm/blob/a734bc1acc0f6ef53a3101c55a5aa9ac6fc2418e/crates/ripr/src/domain/probe.rs#L586)
permits candidate action only for `candidate_current`. Deleted and moved rows
remain in raw evidence. Missing, null, mistyped, unresolved_subject and
unsupported currentness block this bounded candidate comparison, including
on exposed/non-gap rows. They cannot default to current, noncurrent or zero.

`tests/common/currentness_1067.rs` is a test oracle and synthetic native-shape
fixture factory. Synthetic IDs/class breakdowns are deliberately distinct from
the retained analyzer rows; 46/33/30 is a contract control, not an analyzer
replay. All raw rows are retained and all three unknown classifications have
explicit raw counters. Unknowns are separate from gaps. Aggregate suppression
partition reconciliation does not establish per-ID suppression provenance or
change strict-zero policy. Literal ID set differences remain visible; matching
coordinates never create aliases. The line-movement test checks inventory
handling of supplied IDs, not the producer's identity stability.

Admission is explicit: registry:0.10.0 with raw0.2/badge0.6 uses
`LegacyUnspecified0100` (legacy_unspecified for absent currentness); candidate
source a734bc1 with raw0.2/badge0.8/typed0.1 uses `CandidateA734`.
The raw schema0.2 alone cannot distinguish these generations. Source strings
here are asserted fixture context, not executable provenance verification.
No public version/package admission is established. This bounded oracle
supports complete, limitation-free Rust packets only. It rejects partial
outcomes and eol_only_churn as unsupported *by this oracle*, without claiming
that valid producer disclosures are invalid. Full schema8 completeness and
partial lower-bound semantics remain the production owner's separate work.

## Current repository seams

| Seam | Observed boundary | Existing-owner follow-up |
| --- | --- | --- |
| `src/sensors/ripr.rs::ripr_exposure_gap_details_from_value` | Preserves raw gap rows with raw_pre_policy semantics but drops source_currentness; unknowns remain in raw sidecars only | Preserve currentness before advice routing while retaining all raw audit rows; do not equate raw gap total with native current total |
| `src/tools.rs::read_tool_gate_decision` | Counts-only binding accepts unsupported schema with zero count | Exact version/source/schema admission belongs to #1347/#1067 boundaries, not this test oracle |
| `xtask/src/main.rs::validate_local_ripr_outputs` | Exact legacy0.6/0.2 routing; raw-gap/native-partition equality; duplicate IDs currently accepted | After selection, share admitted currentness semantics and reject duplicate IDs; retain legacy rollback mode |
| `scripts/verify-bun-review-artifacts.py::require_ripr_exposure_gap_details` | Exact pinned0.6 envelope rejects0.8 before projection parsing | Migrate native fixtures, projected detail and preserved raw/native parity coherently after selection |
| #813 comparison verifier at03f351a | Historical Python guard excludes only base_deleted | Existing comparison owner retains its versioned successor; Rust controls exclude both deleted and moved from current counts |

Characterization tests deliberately pass when the *existing* boundary is
observed, including loss/unsupported admission and duplicate-ID acceptance.
They are evidence of migration work remaining, not tests claiming those
behaviors are desirable. The owner must replace these expectations when the
corresponding production migration lands. The integration matrix rejects
these malformed comparison inputs independently.

## Reproduce

With Rust1.95.0 and the repository's locked dependencies:

```sh
cargo test --workspace --locked --offline currentness_1067
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo xtask policy-check
```

The selected executor requires the existing locations `RUSTUP_HOME=/workspace/.rustup`,
`CARGO_HOME=/workspace/.cargo`, `PATH=/workspace/.cargo/bin:$PATH` and an isolated
`CARGO_TARGET_DIR=/workspace/ub-currentness-target`. Its initial command-not-found
and read-only default Rust home errors are setup failures, not semantic results.

The historical proposal recorded four isolated compiled oracle experiments
rejected by unchanged tests:
moved counted current; missing defaulted noncurrent; duplicate-ID guard removed;
native-unknown counter guard removed. They mutate only the test oracle, never
the producer or a production parser. Historical logs were executor-local; their receipt hashes do not establish
consumer-side access to those logs. The recovered ZIP retained the patch and
receipt only. Fresh companion validation and exact subjects are recorded in
the Draft handoff.
Restored positive controls must pass after experiments.

## Remaining boundary

#1066 must supply a genuinely qualified immutable selected generation before
#1067 production migration. #1067 owns exact schema/currentness/completeness
plus installer/doctor parity; #1068 owns installed triggering acceptance. The
retained candidate's two current gaps remain above unchanged max0. Neither
synthetic tests nor the historical 26 controls clear full compatibility,
public-package admission, rollback, hosted parity or Required readiness.
No active version mirrors, thresholds, suppressions, policy receipts, workflows,
credentials, production behavior or release state are changed by this proposal.
The companion branch and its GitHub handoff retain test evidence only. Added source/xtask blocks are cfg(test) only.

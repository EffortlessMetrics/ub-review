# PR #1381 bounded frozen comparison

Decision: **comparison_only_no_promotion**. This packet is local, shadow-only, and grants no production or release qualification.

Exact head `9c94d876efcb88e61c74ebeaa099fbfd144ed983`, base `5a72634d4eec98d5de494598902cc6cac256b508`, tree `b5b89cbe0506a3716eac1983120cc618b66558d1`. Only `tests/common/mod.rs` changes: 83 additions, 2 deletions. The frozen 3,798-byte diff SHA256 is `eca0ecec7d330c45392bee8ec9d18f670c4dc9f856efcf6bb2ca5ea83fe253e2`, matching the parent-verified hosted diff digest. Exhaustive exact-tree config, lock, policy and suppression hashes are in frozen-input.json; source.tar retains their bytes.

| Producer | Findings | Gaps | Unknowns | Outcome |
|---|---:|---:|---:|---|
| pinned 0.10.0 | 15 | 2 | 1 | 12 exposed, 2 weak, 1 static unknown |
| owned d646155 | 0 | 0 | 0 | complete no_behavioral_candidates; 1 changed file, 85 changed lines, 0 candidate lines |

Both full raw repeats are byte-deterministic; native badges reconcile with raw counts. Existing binary hashes match the prior receipt, with no rebuild. Complete raw, stderr, invocation and identity files are retained. The read-only receipt verifier accepts all old packets and this new packet. movement.json preserves all 15 stable IDs, subjects, sinks and available currentness; no stable ID remains in candidate. Stable does not emit explicit per-finding currentness, so it is not invented.

Two stable weak rows are `probe:tests_common_mod.rs:error_path:cfb19a36` (capture helper, new line 642) and `probe:tests_common_mod.rs:error_path:46d32c36` (expected-failure helper, new line 682). Both reduce the subject to `})?;` and the sink to artificial error variant `})?`, rather than the whole `command.output().with_context(...)` expression. Both are test-only helpers. Candidate exclusion therefore supports only narrow source-role compatibility; it does not prove either helper exposed.

The actual regression directly invokes both helpers and checks stage, executable, cwd, original I/O root downcast, kind and raw OS error (source lines 985–1021). Stable assertion collection omits the latter kind/raw-OS assertions and includes unrelated same-line assertions; preserve that contradiction rather than accepting a matched line as oracle binding. Parent-owned context-removal and source-flattening mutants compile and fail the intended assertions; clean restored tests pass 2/2. These are owner-verified controls, not independently rerun here: https://github.com/EffortlessMetrics/ub-review/issues/813#issuecomment-6106637123 . This diagnostic patch supplies no ETXTBSY cause or fix claim.

Smallest prerequisite for a *hosted parity* claim here: readable supported materialization of the hosted packet, allowing effective-config/revision-admission comparison. The supported Library transfer failed and was stopped; no hosted bytes or full hosted identity/config parity are claimed. For adoption, this test-role result does not discharge the previously demonstrated Config.policy_errors→exact diagnostic oracle binding prerequisite, the provider cohort's 12 current candidate gaps, or release qualification contracts #1066/#1067/#1068. The parent-reported hosted stable gate remains 2/max0 FAIL with artifact contract PASS.

The original frozen corpus and receipt remain unchanged. No pin, suppression, threshold, production gate, upstream source, release or merge was changed. This additive report is published on the existing comparison branch for owner coordination.

Sealed receipt: Library `libfile_6172a847ce7c8191b35995ffbc04de49`, version `0`, filename `ripr-813-pr1381-frozen-receipt.tar.gz`; 2,371,189 bytes, SHA256 `ce70d49defa6a8bbd062548e9a748f73f2cbe3dac81a3945ecfb3c5a942f9c92`. The Library save succeeded. All 21 packet members were validated against their recorded hashes and lengths. Ownership: `ub-review/tests; #813`, covered by existing `non-rust-fixtures` policy receipt.

# #813 bounded frozen RIPR comparison receipt

Owner: `ub-review/tests`, comparison/qualification lane. Observed 2026-10-11,
Linux x86_64, Rust/Cargo 1.95.0. Decision: **comparison_only_no_promotion**.
No approver, release qualification, protected-gate pass or adoption is claimed.

## Producer and input identities

Stable: crates.io `ripr 0.10.0`, installed locked, default features. Crate SHA256
`d49900ea4daeffc2cef5649b6c7cd4a5a82eaad306fc6f598c79628662f0b786`;
VCS source `ripr@c08d474a92d0edf97ad1e3d444ecb73fc5d439de`, `crates/ripr`.
Local binary SHA256 `49557fab3cb638e2187baedf11acf544a8426681c2215dc7f21c902a4ff9bb08`.

Shadow: `ripr-swarm@d64615529216153eedcd10ba64e4580aa1026ea1`, tree
`b061043c989cdb35d5f7261ae6f3494b4ede03a7`, installed locked from unchanged
source with default features. It reports `ripr 0.11.0 (d64615529216153eedcd10ba64e4580aa1026ea1)`.
Local binary SHA256 `58f8cf301c6fba740966304aaad9655e44c9ab6e495d93f24b1b0f11aacdf3c3`;
Cargo.lock SHA256 `646c5b301c99cd3e0c88c5a7b767fb91fb8a80a67a243365299d69b563eecc16`.
Its [native routed required result](https://github.com/EffortlessMetrics/ripr-swarm/actions/runs/38104595532/job/114377905034)
was verified successful. This justifies a shadow experiment, not selection.
GitHub's latest public `ripr` release observed was [v0.10.0](https://github.com/EffortlessMetrics/ripr/releases/tag/v0.10.0).
A source version or user-reported npm alpha publication is not release proof.

All consumer inputs use base `411d68f9591e56c708e5083278907c0bd8388f96`.

| Corpus | Frozen head | Tree | Diff SHA256 |
| --- | --- | --- | --- |
| #1363 original | d1503cd88f520d7829f8eadbc2cdf5f3693ce2f1 | 8a04a8fe9580474e2f532592e0b90b4073196139 | e2f3d111e5431fb712dff8d3717004ee4ab7e609fa2ad1f6665f251a439f8151 |
| #1363 stronger oracle | bff511ec44fe6948b1c93dcd80c412f8ec3f3c51 | 54aca0ef6883cc0e752874934a9bb788c94b38d2 | 8df1cc85bc38eeafc44106514bd534d9cdf5c7123c6a42ff1a756e22d58a5578 |
| #1366 | a42147fdf60444b19733f7b84d7937cc9ba1dfa8 | 606371593f587086cfc6615f2bb179f1f91ced00 | d2389e355ce162367e627146152cce32b1d8f177b36eba0c754c15e59ddea792 |
| #1368 original | 1c42572f803c900e08de2a6f2d8d4c29ae327045 | 056c4834e9544d1266d89facc0123d01da179c93 | f09c270a6d0f01693a6e7f04855641b526b98dcee719a23f85be083fe2e096a5 |

Source archives, complete tracked-file hashes (including config, policy,
Cargo.lock and suppression ledger), exact diff bytes, all invocations and raw
outputs are retained in the sealed companion receipt. Every tracked input was
hashed before execution and checked unchanged afterward. Each side ran full
raw JSON twice and native badge JSON once, from its same frozen root in ready
mode. No ID aliases, suppressions, max0 thresholds or production gates changed.

## Complete local measurements

All sixteen producer/corpus raw-repeat pairs are byte-identical. All arrays,
classification totals, native denominators, typed outcome counts and candidate
input identities reconcile. Candidate raw/badge outcomes agree; no run or typed
output limitations were declared. Stable raw has schema0.2/badge0.6; candidate
raw has schema0.2/badge0.8 with explicit source-currentness and typed outcomes.

| Corpus | Stable findings/gaps/unknowns | Candidate findings/native gaps/unknowns |
| --- | --- | --- |
| #1363 original | 42 / 1 / 7 | 20 / 0 / 20 |
| #1363 stronger | 42 / 1 / 7 | 20 / 0 / 20 |
| #1366 | 208 / 3 / 79 | 25 / 12 / 12 |
| #1368 original | 319 / 130 / 53 | 0 / 0 / 0 |
| helper-exact | 8 / 5 / 3 | 1 / 1 / 0 |
| helper-no-oracle | 6 / 4 / 2 | 1 / 1 / 0 |
| helper-self-expected | 8 / 5 / 3 | 1 / 1 / 0 |
| production-lookalike | 8 / 5 / 3 | 4 / 3 / 1 |

All native suppressed counts are zero. #1366 candidate has thirteen raw weak
rows: twelve `candidate_current`, one `base_deleted`; the native current count
is twelve. That is calibrated source-side accounting, not missing output.
Zero native gaps in config do not mean exposure: **all twenty findings are
unknown** (4 infection, 15 propagation, 1 static). #1368 says
`no_behavioral_candidates`, with two changed files/207 changed lines, zero
candidate lines/probes; it makes no runtime-test adequacy claim.

Raw output sizes range from 1,594 to 132,993,300 bytes on consumer inputs.
Raw invocation durations were 0.319–2.583 seconds; these local observations are
not a benchmark. Candidate raw rendering records an 8MiB allowance; stable and
badge rendering do not claim support for it. A separate 128MiB per-stream
kernel bound applied to consumer runs. Complete output was retained, never
replaced with a displayed subset. Ten corrupted receipt controls were rejected
(count/ID/class mismatch, limits/non-consumability, badge denominator, typed
outcome count/input/limitations and raw/badge disagreement); valid controls pass.

## Source role, whole subject and oracle ledger

**#1363:** stable `probe:src_config.rs:field_construction:3064df08` is closing
punctuation `),` at1132, with a fabricated struct-field sink `)`. The stronger
exact diagnostic assertion is already in its raw assertion texts, yet the row
stays weak. Candidate replaces that fragment with the complete production
`detail: format!(...)` field at1130 (`field_construction:c403dbee`) and complete
`errors.push(PolicyError { ... })` call/effect subjects at1128. These remain
`propagation_unknown`, with `helper_result_not_forwarded` and unconfirmed field
observation. Their IDs are distinct semantic subjects, not aliases or exposure
credit. Caller/output lineage is `sanitize_providers_section(&mut table,
&mut errors)` → `PolicyError.detail` → `Config::from_toml_with_policy_receipts`
→ `malformed.policy_errors[0].detail`.

Compiled diagnostic-removal control, replacing only the production format text
with `diagnostic removed`: original-head test passes; stronger-head test fails
at the independent exact expected text. Stronger pristine test passes. Consumer
package outputs were cleared before each final mutant build; earlier cached
cross-worktree attempts are retained as invalid control evidence. The candidate
still cannot distinguish this actual oracle strengthening through its field/
mutable-output propagation. Unknown is retained, not renamed exposed.

**#1368:** all130 stable gaps are in test-only
`src/lane_packets_lifecycle_tests.rs`. Its parent `#[cfg(test)]`/`#[path]` module
is evidence context. Candidate admits no production subjects from this exact
input. The production-lookalike control removes only the cfg guard while
keeping the same helper filename: candidate retains three helper-file subjects
plus the production return. Filename exclusion is therefore not the explanation.
The [owner's strengthened lifecycle receipt](https://github.com/EffortlessMetrics/ub-review/blob/e4f2785e123717ce4c0c8585dbb563c63d638b15/docs/ci/ripr-lifecycle-regression-1368.md)
separately records the real missing-reap oracle and killed omit-reap/omit-kill/
false-success controls. That stronger head is not substituted for this original
corpus. Removing test subjects cannot certify their oracles as adequate.

**Helper value binding:** all four pristine fixture tests pass. Changing only
production `answer` from7 to8 fails exact-helper and production-lookalike tests;
no-oracle and self-computed expectation tests still pass. Candidate keeps the
production return `probe:src_lib.rs:return_value:b4958fd8` (`7`): exact-helper is
weak with `observation_unverified` and incomplete owner-bound propagation;
no-oracle is reachable-unrevealed; self-computed is weak. It preserves both
negative controls, but does not award supported exposure to the actually
independent equality oracle. Its helper/assignment/value-binding limitation
remains concrete. This does not admit arbitrary ensure helpers.

**#1366 stable obligations:** production enum guard9ba81bec maps to candidate
predicate912e5c21 at101, now `infection_unknown`; strong output tests do not
establish supported enum-boundary infection. The removed test helper
`spawn_fake_openai_provider(2)?` and test punctuation `)?;` have no candidate
production rows. Their legacy new-side coordinates must not be described as
current text or a stale binary. The [owner's exact source and predicate controls](https://github.com/EffortlessMetrics/ub-review/issues/813#issuecomment-6105342463)
remain authoritative behavioral evidence; this lane makes no blanket clearance.

All twelve candidate-current weak rows remain unresolved, with raw flow,
assertions and related-test records retained:

| ID family/suffix (prefix `probe:src_model_api.rs:` unless stated) | Line / subject | Actual oracle route and remaining boundary |
| --- | --- | --- |
| field_construction:a6b1af79 / 68a39ac1 | 97/98 Accept/Content-Type vector elements | unit contains assertions591–592; actual HTTP header equality4344–4351; producer `oracle_confirmation_mixed` |
| field_construction:1a467983 |99 versioned User-Agent | actual request equality4340–4343; producer `observation_unverified` |
| call_deletion:782b1045 |104 process nonce initialization | session isolation/cross-process owner controls; exact changed expression unconfirmed |
| call_deletion:db358889 / error_path:ec191693 |106 canonicalization/context propagation | missing-directory test598–612; complete fallible-expression ownership and diagnostic binding unresolved |
| call_deletion:4e9fee8a / 66561632 / 747f4c8e / 32283fd7 |108–111 digest updates | session stability/isolation/shape and nonce/directory controls; per-update oracle semantics are not universally proved |
| return_value:8f773a8d |114 Ok(headers) | unit and HTTP consumers observe headers; producer broad-error discriminator remains weak |
| `probe:src_model_exec.rs:error_path:c6a8295d` |1675 model_request_headers(...)? | actual transport tests reach call; error propagation/oracle expression binding unconfirmed |

Additional raw `probe:src_model_exec.rs:field_construction:a6d7f179` at1683 is
explicitly `base_deleted` (old literal header slice). It stays in audit output,
outside the twelve current native gaps. Vector elements must not be silently
accepted as struct-field subjects; nonce/digest/control successes do not clear
every candidate obligation. Reuse producer subject/error/Result owners; shared
implementation belongs to ripr-swarm, outside this lane's write scope.

## Qualification boundary and next prerequisite

The smallest demonstrated **source-proof** prerequisite is owner-bound
propagation/observation for the stronger config diagnostic field through the
mutable error ledger, calibrated by the unchanged-expectation compiled control.
Whole-field subject mapping improved; output binding is still unresolved.
The minimal helper pair independently pins the assignment-to-value seam.
No single repair here is claimed to clear the twelve provider gaps.

The immediate **adoption** stop is an unaccepted #813 compatibility decision:
this candidate has twelve current provider gaps, twenty unknown config subjects,
and a failed positive helper discriminator. #1066 also lacks a qualified public
immutable candidate version/checksum/schema bundle. #1067 must separately admit
badge0.8 and currentness/completeness semantics; #1068 needs installed triggering,
clean and invalid-tool proof. Cross-platform identity, line movement, changed
expression identity, suppression continuity and rollback were not qualified in
this Linux lane. A released baseline is not automatically a compatible rollback.

Hosted artifacts11691478526/11690543290/11690569464 were resolved but supported
Library materialization failed here; these are new local receipts, not hosted
byte-parity claims. Correct #1370 artifact metadata is11690717877 for run38109954163.
#1361/#1370 and newer strengthened heads remain separate owner corpora, not
silently omitted passing cases. #1314/#1348 evidence is preserved; #1348's27→32
negative decision is not superseded. No upstream source write, pin, suppression,
threshold, workflow, production gate, release, or merge occurred.

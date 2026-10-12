# #1401 source-role case prepared for #813 adjudication

Owner: `ub-review/tests`, existing #813 comparison lane. Observed 2026-10-11
23:58 UTC on Linux x86_64, Rust/Cargo1.95.0. Decision remains
**comparison_only_no_promotion**. Case approver is null. No public candidate,
installed-release, full compatibility, Required-gate pass or adoption is claimed.

## Reused real-consumer evidence

The exact #1401 replay and its three compiled fault controls are reused from
[consumer receipt6115232166](https://github.com/EffortlessMetrics/ub-review/pull/1401#issuecomment-6115232166)
and [canonical source-role owner#3213 receipt6115227671](https://github.com/EffortlessMetrics/ripr-swarm/issues/3213#issuecomment-6115227671).

Consumer base777d86a0fef6702f76502dd1c959c97d43600682 →
head93fd1e4701f8f5e20571628eda55c639fec5d3b4;
treee5a160d72462e0ab18dd9c56e05e6317c63ac169;
patch SHA25669d819534ecee4afa81ab335176327b96dbd0c916fe3cccb51b27f42400914a6.
Registry0.10.0 reports the two test-helper return rows3a1394a4/f39e97b9;
source75ecb7fd omits them as test evidence, not exposed returns. The nonempty
production control and cfg(test) control remain1/0. The source's named
`tests/common/cli_lock.rs:rust_module_ambiguous_parent` remains disclosed.
Hosted gate38182248251 stays FAIL2/max0; local reconstruction is not downloaded
hosted per-row or binary parity. None of those executions were repeated here.

## Only the missing source-role discrimination was run

The helper prefix is copied byte-for-byte from #1401's original
`tests/common/executable_fixture.rs`, through the complete `FixtureCommandExt`
implementation, before the executable writer. Its1759-byte SHA256 is
`e04650b6cf3ee9d8f29d804a3fb78ec79469fc8ece749553902960c637595df9`.
It retains the launch reader, `fixture_spawn`, complete piped capture chain and
status method. No claim about the omitted writer/ETXTBSY implementation follows.
All four snapshots contain identical helper bytes and compile/run one actual
capture test with literal stdout, empty stderr and native status7 assertions.
They are unpublished, standalone packages; no active config or policy changed.

| Added control | Declared role | Probes/findings | Native gaps | Unknowns |
| --- | --- | ---: | ---: | ---: |
| test-layout | tests/common module reached from integration test |0|0|0|
| cfg-test-module | same src helper reached through cfg(test) |0|0|0|
| production-normal | unconditional exported src module |14|7|7|
| production-test-name | unconditional exported src/executable_fixture_tests.rs |14|7|7|

Each case ran actual `check --root . --diff diff.patch --mode ready` twice as
JSON and once as badge-json. All12 invocations exited0; repeated raw outputs
were byte-identical; full raw/native/typed counts and exact patch identities
pass the existing read-only receipt verifier. Unknown categories remain explicit.
Production classifications are2 weakly_exposed,1 reachable_unrevealed,
4 no_static_path and7 static_unknown, with zero exposed and zero suppressions.
Zero test-role findings means no production subjects; it does not certify tests.

The exact complete capture-chain production subject remains visible:

- normal filename: `probe:src_executable_fixture.rs:return_value:ea86593b`;
- test-like filename: `probe:src_executable_fixture_tests.rs:return_value:4c0141a1`.

Both are weakly_exposed at line38 with the same whole return expression:
`self.stdin(...).stdout(...).stderr(...).fixture_spawn()?.wait_with_output()`.
These identities are distinct path-bound subjects, not aliases for the two
legacy test-only rows. Neither gets return-observation credit. The corresponding
spawn and status returns also remain visible as no_static_path; they are not
silently hidden. This specifically rejects “remove every analogous helper”
and filename-only exclusion explanations for the recorded source-role result.

## Producer and retained artifact boundaries

The original real-patch receipt used source75ecb7fd/tree60d408f81071da60aa4f5000cf9e78a71dd69d2f,
workspace binarya9b84696866b48c7ee0db867dfbba9fb1a8e965178d83b748ae22891c43a9874.
That binary was not available in this executor. These additional controls use a
**new separately identified workspace shadow build** from the unchanged exact
same source/tree, default features, `cargo build -p ripr --bin ripr --locked`.
Binary SHA256 is
`79ad5381ee8973228cfa2651c172c7d9ebc94ca74ec8cab73d303bc25cfcce73`;
Cargo.lock SHA256646c5b301c99cd3e0c88c5a7b767fb91fb8a80a67a243365299d69b563eecc16.
Its version probe reports full75ecb7fd. Source checkout remained clean. The first
offline build failed on missing oxc_allocator cache; the same locked online build
then completed. This is not the earlier executable, immutable package/install,
public release, or binary-equivalence proof. Only the source-role controls use it.

Raw/stdout/stderr/invocation/frozen-input records are under `evidence/`;
`results.json` retains every classification and subject. Runtime logs and producer
identity are retained alongside the snapshots. `FILE_MANIFEST.json` binds all
new packet files. The original four-case corpus/manifest and PR1381 receipt are
unchanged. Fixture additions remain under existing non-rust-fixtures ownership;
the existing receipt verifier is reused without changing its implementation.

Run the existing auditor and the additive independent claim guards:

```text
python3 fixtures/ripr-shadow-813/receipt-verifier/verify.py fixtures/ripr-shadow-813/source-role-1401/evidence
python3 -m unittest discover -s fixtures/ripr-shadow-813/receipt-verifier -p 'test_*.py' -v
```

All16 claim-guard tests pass. New data-only negative controls reject a fabricated
all-zero production denominator even when unknown accounting reconciles, and
reject a production report that replaces the capture subject with an unrelated
return while preserving counts. These are receipt-oracle controls, not compiled
producer mutations. Four native fixture tests each ran1/PASS. Linux-only command
fixtures do not establish Windows/platform parity. No other frozen comparison
corpus, baseline gate or published analyzer was rerun.

## Exact owner decision still needed

Prepared evidence satisfies the bounded source-role discriminator; it is **not
marked accepted by a compatibility approver**. The existing #813 maintainer must
adjudicate this exact packet as an additive `should_stay_quiet` test-role case
plus `should_stay_visible` production counterparts for source75ecb7fd, explicitly
without exposed-return credit. The decision must retain the seven production
unknowns and unresolved propagation/reach subjects, the real-case named module
limitation, the separate source-built binary identity, and the frozen original
consumer/patch identities. Independent review may reject or limit that case.

This decision cannot accept the whole candidate, erase the existing provider12,
config20unknown/exact-helper-positive obligations, or stand in for identity,
suppression, cross-platform, compatible installed rollback and public artifact
acceptance. Those retain #813/#2769/#1066/#1067/#1068 and upstream owners. #1389
is unchanged at4fb9ef4005c076cd7e8c7ab4f4e34a5566681efe; no selection/promoter,
production code, pin, threshold, suppression, release, source sync or merge changed.

# Frozen role and oracle controls for #813

Owner: `ub-review/tests`, bounded comparison/qualification lane under #813.
These snapshots have no gate, suppression, installer or analyzer authority.
They preserve the #1368 owner's minimal fallible-helper reproducer and add
three adversarial controls. Existing #1314's frozen #1306 comparison is separate.

Each directory is a standalone unpublished Cargo package with a complete
added-file `diff.patch`. Both analyzers must consume the same source, patch,
manifest and lockfile bytes. The released baseline is `ripr 0.10.0`; the
source-tested shadow candidate is `ripr-swarm@d64615529216153eedcd10ba64e4580aa1026ea1`
(tree `b061043c989cdb35d5f7261ae6f3494b4ede03a7`). This source identity is not
an immutable public release selection or promotion approval.

| Snapshot | Actual oracle | Wrong answer 7→8 | Role expectation |
| --- | --- | --- | --- |
| helper-exact | `ensure!(actual == 7)` in called helper | test fails | cfg(test) module evidence remains indexed; helper is not a production subject |
| helper-no-oracle | calls production function and discards output | test passes | changed production return remains unproved |
| helper-self-expected | compares actual to another call of changed function | test passes | self-computed expectation must not clear the production return |
| production-lookalike | same helper filename and exact oracle, unguarded module declaration | test fails | filename alone must not exclude production helper subjects |

For each snapshot, run `cargo test --locked --manifest-path <snapshot>/Cargo.toml`.
Copy it to a disposable directory, replace only `answer() -> u8 { 7 }` with
`answer() -> u8 { 8 }`, and repeat. Keep the oracle unchanged. A compile failure
or zero selected tests does not count as a killed control.

Analyzer replay from the snapshot directory:

```sh
ripr check --root . --diff diff.patch --mode ready --format json
ripr check --root . --diff diff.patch --mode ready --format badge-json
```

Run full raw JSON twice per producer and preserve all raw rows, unknowns,
source-currentness and native badge counts. The candidate's diagnostic render
allowance may be explicitly recorded as `RIPR_CHECK_FINDINGS_BYTES=8388608`;
it changes neither analysis scope nor required threshold. Reject limited,
non-consumable or count-inconsistent output. The released producer does not
support that allowance; do not label it effective there or for badge rendering.

Runtime controls calibrate only this changed return, not universal mutation
adequacy. A lower total or excluded helper rows alone is not success. Missing
helper relation, assignment/value binding, subject span, or unsupported oracle
must stay an explicit limitation. Whole-expression punctuation, complete
consumer corpora, platform identity, schema migration and rollback remain
separate qualification obligations. In particular #1368's original lifecycle
tests missed a reap control; strengthened owner head e4f2785 preserves that real
repair separately from original frozen head 1c42572.

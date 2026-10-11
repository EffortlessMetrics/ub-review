# PR #1368 subprocess lifecycle proof and RIPR attribution

This receipt separates a corrected runtime-test blind spot from a reproduced
analyzer limitation. It does not qualify an analyzer, waive the strict-zero
product gate, or authorize a suppression or pin change.

## Frozen consumer input

- Consumer head: `1c42572f803c900e08de2a6f2d8d4c29ae327045`.
- Base: `411d68f9591e56c708e5083278907c0bd8388f96`.
- Hosted run: [38109874366](https://github.com/EffortlessMetrics/ub-review/actions/runs/38109874366).
- Hosted artifact: `11690569464`, digest
  `sha256:557c09260aa15cbd3e4b059982061af852486118fb68de71c603c19d047f22d9`.
- Parent independently verified the hosted artifact contract. Local artifact
  download was unavailable; local analysis is not a claim of hosted byte parity.
- Diff SHA-256: `f09c270a6d0f01693a6e7f04855641b526b98dcee719a23f85be083fe2e096a5`.
- Analyzer: crates.io `ripr 0.10.0`, installed with `cargo install ripr --locked
  --version 0.10.0`; local executable SHA-256
  `a11bacd678aaf90e3b3008800256b749e5800144c5906e1d33d62034784758a7`.

Reproduce from an untouched checkout of the consumer head:

```sh
git diff --binary --no-ext-diff 411d68f9591e56c708e5083278907c0bd8388f96 1c42572f803c900e08de2a6f2d8d4c29ae327045 -- > frozen.diff
ripr check --root . --diff frozen.diff --mode ready --format json > detail.json
ripr check --root . --diff frozen.diff --mode ready --format badge-json > badge.json
```

Two local detail runs in the same checkout were byte-identical. There were 319
findings: 136 exposed, 103 reachable-unrevealed, 27 weakly-exposed, 8
infection-unknown, and 45 static-unknown. The badge reported 130 unsuppressed
exposure gaps and zero suppressed gaps. All 130 gap rows belonged to
`src/lane_packets_lifecycle_tests.rs`. Gap families: 40 call-deletion, 30
return-value, 28 error-path, 26 field-construction, and 6 match-arm.

For example, `probe:src_lane_packets_lifecycle_tests.rs:field_construction:2504077a`
probes fixture `timeout_sec: Some(1),` at line 42. It relates both actual tests by
`direct_owner_call`, but assigns them unknown oracle kind and no strength. Their
recorded text is only the call to `stopped_child_receipt`, not its `ensure!`
oracles. `field_construction:2c6e179b` probes fixture revision semantics at line
31 and additionally associates unrelated tests through weak token substrings.
These are evidence-helper subjects, not changed production execution behavior.

## Compiled runtime controls and genuine repair

The original two tests passed when the child's `reap_process` implementation was
changed from `self.wait().map(|_| ())` to `Ok(())`. Absence of a delayed marker
proves a child cannot finish its work; it does not prove a killed child was
reaped. The regression now records the live child's PID and verifies its procfs
entry disappears before the runner returns on Linux. It first requires that the
live child is visible in procfs. Other platforms retain the original physical
marker, receipt, and ledger checks without claiming this additional reap proof.

Isolated compiled controls against the strengthened tests:

| Runtime mutation | Result |
| --- | --- |
| Skip child reap | Both tests fail: `proof runner returned before reaping child` |
| Skip child kill, still wait | Both tests fail when cleanup observes the delayed marker |
| Record runner interruption as `Succeeded` instead of `Cancelled` | Interruption test fails: `physical interruption must retain its terminal disposition`; timeout control still passes |
| Unmodified runtime | Both tests pass |

No runtime mutant is retained in the product branch. Control builds used a
separate Cargo target directory to keep mutated binaries out of normal checks.
The real repair changes an oracle, not analyzer-matcher spelling. The strengthened
working-tree diff increases pinned local gaps to 143 (116 reachable-unrevealed,
27 weakly-exposed), all still in the test-only helper file. It adds runtime
discrimination and does not clear the analyzer blocker.

## Minimal frozen oracle reproducer

Create an isolated package with edition 2024 and `anyhow = "=1.0.100"`. Its
`src/lib.rs` is:

```rust
pub fn answer() -> u8 { 7 }

#[cfg(test)]
#[path = "test_support.rs"]
mod tests;
```

Its `src/test_support.rs` is:

```rust
use anyhow::{Result, ensure};
fn check_answer() -> Result<()> {
let actual = super::answer();
ensure!(actual == 7);
Ok(())
}

#[test]
fn observes_answer() -> Result<()> { check_answer() }
```

Create an added-file diff for both Rust files, then run the same pinned `ready`
JSON command. This produces 8 findings, including 5 reachable-unrevealed gaps.
A compiled `cargo test` passes. Changing only `answer`'s returned value from 7 to
8 makes that same test fail with an `anyhow::ensure!` error.

Diagnostic variants were compiled separately; they are not replacement product
oracles:

| Shape | Baseline / wrong-answer exit | RIPR result |
| --- | --- | --- |
| Helper `ensure!(actual == 7)` | 0 / 101 | 5 reachable-unrevealed, 1 infection-unknown, 2 static-unknown |
| Helper `assert!(actual == 7)` | 0 / 101 | 4 reachable-unrevealed, 1 infection-unknown, 2 static-unknown |
| Direct test `ensure!(actual == 7)` | 0 / 101 | 1 reachable-unrevealed, 1 static-unknown |
| Direct test `assert!(actual == 7)` | 0 / 101 | 1 weakly-exposed (`observation_unverified`), 1 static-unknown |

The helper variants place either oracle in `check_answer`. The direct variants
put `let actual = super::answer();` and the oracle directly in the `#[test]`
function inside an inline test module. Even the direct standard assertion does
not clear the actual return-value subject: RIPR does not establish the
assignment-to-observed-value relationship. A spelling substitution is therefore
neither a correct repair nor a qualifying analyzer comparison.

## Producer repair and consumer qualification

[EffortlessMetrics/ripr#1453](https://github.com/EffortlessMetrics/ripr/issues/1453)
owns test-only subject exclusion, new-module discriminator relationships, and
fallible-oracle recognition. The released crate's relevant components are:

- `analysis/probes/diff.rs::changed_line_owned_by_test`: excludes directly
  annotated test functions, not helpers admitted through a test-only module.
- `analysis/syntax/ra.rs::summarize_file`: extracts oracle facts only when the
  function itself has the test attribute; parent `cfg(test)`/external-module
  ownership is not propagated into the helper's evidence role.
- `analysis/syntax/ra.rs::is_assertion_macro` and
  `analysis/extract/oracles/scan.rs::is_assertion_line`: omit `ensure!`.
- `analysis/classify/related_tests.rs::find_related_tests`: relates caller tests,
  but does not transport the helper's oracle/value facts through that call;
  broad token matches can attach unrelated oracles to fixture fields.

The producer needs fixtures proving test-only helper/declaration subjects do not
become production probes, supported fallible oracles retain their observation
and discriminator semantics, helper/assignment relationships are resolved or
named as limited, and genuine smoke/no-oracle controls remain weak. Runtime
negative controls calibrate only the exercised defects, not universal mutation
adequacy.

[ub-review#813](https://github.com/EffortlessMetrics/ub-review/issues/813) must
compare the released baseline and an identified released candidate against
byte-identical frozen head/diff/config/ledger inputs and these controls. Retain
all finding identities, classifications, schema/suppression changes, gate
impact, determinism, supported platform parity, duration/size, and rollback
receipts. Promotion needs the accepted compatibility artifact and a separately
reviewed coherent pin change. A local reduction or upstream issue closure cannot
clear PR #1368's required product gate.

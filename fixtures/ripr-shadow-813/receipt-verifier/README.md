# Comparison receipt claim guards

Owner: `ub-review/tests`, #813. This adapts the existing data-only comparison
packet auditor into a read-only, reusable verifier. It does not install or run
an analyzer, parse active product sensor output, change a pin, or qualify a gate.

Run against the existing uncompressed comparison directory:

```sh
python3 fixtures/ripr-shadow-813/receipt-verifier/verify.py <evidence-root>
python3 -m unittest discover -s fixtures/ripr-shadow-813/receipt-verifier -p test_verify.py -v
```

The directory contains `packets/`, `fixture-packets/`, raw stdout/stderr and
invocation receipts. Its original comparison contract is `manifest.json`, or
`sealed/manifest.json` in the original local layout. Compressed archive members
must be materialized/decompressed separately; this verifier is not an archive
extractor. It emits validation diagnostics to stdout and never rewrites input,
projection, summary or manifest files.

A zero native gap count may coexist with unknowns. Every raw unknown category,
including zero categories, must be explicit and reconcile with retained rows.
Native unknown counts must retain current unknowns; base-deleted unknowns remain
explicit in the raw breakdown rather than being discarded. Missing, zeroed,
or boolean unknown counts fail. The accepted comparison contract is exactly
`shadow-only` / `comparison_only_no_promotion`; promotion decisions, required
authority, or true qualification/promotion/required-pass flags fail.

Validation on the existing frozen packet: all sixteen producer/corpus pairs
accepted; all354 existing evidence files were hash-checked unchanged, including
the original sealed archive. Fourteen focused tests pass. Removing only the
unknown guard makes six negative controls fail; removing only the promotion
guard makes six negative assertions fail. Both mutants exit1; pristine exits0.
The three qualification-flag subcases contribute separately to the latter six
failures. These controls validate the receipt boundary, not producer behavior,
runtime mutation adequacy or promotion readiness.

# RIPR suppression inventory

This is the bounded inventory receipt for issue #872. It records what the
repository can establish from the suppression ledger and a pinned RIPR
0.10.0 detail artifact; it is not a semantic approval of every suppression.

Run from clean main `26b1094c2268ae10f2c73463e9c71d78664afae0` plus the
inventory change, using:

```text
cargo --locked xtask ripr --base 26b1094
cargo --locked xtask ripr-inventory --artifact-dir target/xtask/ripr
```

For hosted currentness, retrieve the exact artifact for the reviewed head and
run the inventory against the extracted `sensors/ripr` directory:

```text
gh run view <run-id> --json headSha --jq .headSha
# Require the output to equal the reviewed PR head SHA.
gh run download <run-id> --name ub-review-gate --dir target/hosted-ripr
python scripts/verify-bun-review-artifacts.py target/hosted-ripr \
  --expected-review-profile ub-review-self --expected-repo-kind ub-review
cargo --locked xtask ripr-inventory --artifact-dir target/hosted-ripr/sensors/ripr
```

The command must find both `exposure-gaps.ripr.stdout` and
`exposure-gaps.ripr.stderr` beside `exposure-gaps.json`; a missing sidecar or
head-SHA mismatch makes currentness unknown.

The detail input must be the complete `ub-review.ripr_exposure_gaps.v3`
artifact from the exact hosted head. v2, truncated, malformed, or
detail-unavailable artifacts are classified as unknown currentness rather than
evidence that a suppression is unmatched.

Observed counts from hosted run `31822364107` at exact head
`9c26591bfa6d70b02fa68d1a47213ef747257010` (2026-08-14):

| classification | before | after | meaning |
|---|---:|---:|---|
| malformed | 1 | 1 | one ledger record could not be parsed as an independent suppression row |
| duplicate | 0 | 0 | no mechanically proven duplicate row was removed |
| matched_current_diff | 0 | 0 | no valid ledger ID appeared in this current detail artifact |
| unmatched_by_current_diff | 3639 | 3639 | valid ledger IDs absent from this exact hosted v3 finding set |
| unknown_currentness | 0 | 0 | complete v3 detail and both raw sidecars were present |

The inventory made no ledger edits and did not renew the overdue
`non-rust-ripr-suppressions` policy receipt. The malformed count requires a
separate review of the surrounding ledger text; it is intentionally not
auto-repaired. Content-addressed unmatched IDs require semantic evidence or a
representative-diff procedure before removal or renewal. A validated
production-form `sensors/ripr/exposure-gaps.json` artifact is required before
any current-diff match claim. The hosted detail contained 31 complete stable
IDs, with gate counts `unsuppressed_exposure_gaps = 31` and
`suppressed_exposure_gaps = 0`; the gate was red, so these are diagnostic
currentness evidence rather than a merge or suppression approval.

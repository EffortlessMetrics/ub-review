# RIPR suppression inventory

This is the bounded inventory receipt for issue #872. It records what the
repository can establish from the suppression ledger and a pinned RIPR
0.10.0 detail artifact; it is not a semantic approval of every suppression.

Run from clean main `26b1094c2268ae10f2c73463e9c71d78664afae0` plus the
inventory change, using:

```text
cargo --locked xtask ripr --base 26b1094
cargo --locked xtask ripr-inventory --artifact-dir target/xtask/ripr
# Without --provenance/--reviewed-head, valid rows remain unknown_currentness.
```

For hosted currentness, retrieve the exact artifact for the reviewed head and
run the inventory against the extracted `sensors/ripr` directory:

```text
gh run view <run-id> --json headSha --jq .headSha
# Require the output to equal the reviewed PR head SHA.
gh run download <run-id> --name ub-review-gate --dir target/hosted-ripr
python scripts/verify-bun-review-artifacts.py target/hosted-ripr \
  --expected-review-profile ub-review-self --expected-repo-kind ub-review
cargo --locked xtask ripr-inventory --artifact-dir target/hosted-ripr/sensors/ripr \
  --provenance target/hosted-ripr/ripr-provenance.json \
  --reviewed-head <reviewed-head-sha>
```

The command must find both `exposure-gaps.ripr.stdout` and
`exposure-gaps.ripr.stderr` beside `exposure-gaps.json`; a missing sidecar or
head-SHA mismatch makes currentness unknown.
The provenance manifest must contain non-empty `reviewed_head`, `run_id`, and
`diff` fields; `reviewed_head` must equal `--reviewed-head`. Missing or stale
provenance is classified as unknown currentness.

The detail input must be the complete `ub-review.ripr_exposure_gaps.v3`
artifact from the exact hosted head. v2, truncated, malformed, or
detail-unavailable artifacts are classified as unknown currentness rather than
evidence that a suppression is unmatched.

Historical evidence from hosted run `31842460778` at exact head
`4d6707334c4753aa42420df18a431405747373ec` (2026-08-14) recorded 54 RIPR
new-unsuppressed findings; the gate was red, so this is not current merge proof.

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
any current-diff match claim. The historical packet recorded 54
new-unsuppressed findings; historical results are diagnostic evidence only, not
a merge or suppression approval. No current hosted proof is claimed.

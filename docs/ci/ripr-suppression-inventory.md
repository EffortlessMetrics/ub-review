# RIPR suppression inventory

This is the bounded inventory receipt for issue #872. It records what the
repository can establish from the suppression ledger and a pinned RIPR
0.10.0 detail artifact; it is not a semantic approval of every suppression.

Run from clean main `96e1b1dbb2d6508cc8ee35dcc5402684b722687b` plus the
inventory change, using:

```text
cargo xtask ripr --base 96e1b1d
cargo xtask ripr-inventory --artifact-dir target/xtask/ripr
```

Observed counts (2026-08-13):

| classification | before | after | meaning |
|---|---:|---:|---|
| malformed | 1 | 1 | one ledger record could not be parsed as an independent suppression row |
| duplicate | 0 | 0 | no mechanically proven duplicate row was removed |
| matched_current_diff | 0 | 0 | no valid ledger ID appeared in this current detail artifact |
| unmatched_by_current_diff | 0 | 0 | invalid local/raw detail is never treated as evidence of absence |
| unknown_currentness | 3639 | 3639 | local raw output is not the production detail contract |

The inventory made no ledger edits and did not renew the overdue
`non-rust-ripr-suppressions` policy receipt. The malformed count requires a
separate review of the surrounding ledger text; it is intentionally not
auto-repaired. Content-addressed unmatched IDs require semantic evidence or a
representative-diff procedure before removal or renewal. A validated
production-form `sensors/ripr/exposure-gaps.json` artifact is required before
any current-diff match claim.

# GitHub-facing review goldens

`src/tests/review_golden.rs` runs six representative review shapes through the
production review compiler and records the complete GitHub-facing surface under
`fixtures/review-golden/`.

The PR body is recorded verbatim. Every inline comment runs through
`github_review_post_comment_body`, so each fixture includes the same lane-prefix
stripping and suggestion-fence rendering used by delivery. Artifact-side lane
provenance remains present in the compiled review; it is deliberately absent
from the posted snapshot.

The proof fixtures use the same coherent red/green shape as production:
`focused-red-green`, `base-plus-tests`, a passing HEAD command, and a passing or
failing base-plus-tests command consistent with the final result. A fixture may
not label an incomplete or contradictory packet as discriminating.

## Cases

- `clean-no-findings`: meaningful investigation, no public review.
- `one-inline-finding`: one concise source-local action.
- `inline-and-summary-finding`: an anchored defect plus a distinct cross-cutting
  concern.
- `evidence-gap`: HEAD and base+tests both pass, so the changed test is
  non-discriminating.
- `test-proof-and-verification`: HEAD passes and base+tests fails.
- `inline-suggestion`: exact GitHub suggestion rendering.

Refresh only after reviewing the output diff:

```text
UB_REVIEW_BLESS=1 cargo test --locked --bin ub-review review_golden -- --test-threads=1
cargo test --locked --bin ub-review review_golden -- --test-threads=1
```

These snapshots are descriptive evidence. They expose duplication, ordering,
grammar, accidental machinery, and delivery-transform drift; they do not make
current wording correct merely because it is recorded.

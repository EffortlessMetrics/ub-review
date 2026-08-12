# Compiled review payload goldens

`src/tests.rs` runs six representative review shapes through the
production review compiler and records its pre-delivery payload values under
`fixtures/review-golden/`.

The PR body is recorded verbatim. Prepared reviews first pass production post
validation against the fixture patch's real RIGHT-side diff lines, then run
through `github_review_post_payload`. Each fixture records that serialized
pre-delivery payload, including lane-prefix stripping and suggestion-fence
rendering. Network delivery is excluded: it can add request metadata such as a
commit id and controls final submission. Artifact-side lane provenance remains
present in the compiled review and absent from the transformed comment body.

The harness characterizes compiler-to-post-payload behavior. Reporter
distillation and the network POST are intentionally outside this fixture seam.

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
- `inline-suggestion`: pre-delivery suggestion rendering.

Refresh only after reviewing the output diff:

```text
UB_REVIEW_BLESS=1 cargo test --locked --bin ub-review review_golden -- --test-threads=1
cargo test --locked --bin ub-review review_golden -- --test-threads=1
```

Blessing is fail closed: only the exact value `UB_REVIEW_BLESS=1` writes
fixtures. CI and ordinary local proof run without that variable and therefore
compare bytes without modifying the snapshots. Always follow a bless pass with
the non-bless command above so the refreshed files prove idempotent immediately.

These snapshots are descriptive evidence. They expose duplication, ordering,
grammar, accidental machinery, and pre-delivery transform drift; they do not make
current wording correct merely because it is recorded.

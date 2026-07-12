# Review-experience fixtures

`fixtures/review-experience/perl-lsp-3627.json` is the first golden
end-to-end case for the public review contract. It records the real PR
conversation shape, current-head transition, structural claims, existing
external-review threads, deterministic proof receipts, and the expected narrow
human surface.

The fixture is deliberately independent of model wording and GitHub transport.
It is a contract test for the boundaries that must survive implementation:

- claims with shared vocabulary remain distinct by structural identity;
- an existing adequate thread is reused instead of duplicated;
- current-head fixes invalidate old review surfaces and produce silence;
- only current-head human-facing locations are eligible for delivery; and
- planner, lane, skipped-proof, and unrelated workspace language stays out of
  public finding text.

Run the focused proof with:

```text
cargo test --locked review_experience::tests::perl_lsp_3627
```

This fixture is the regression boundary for claim-graph integration,
receipt-driven replanning, and transactional inline delivery. Its test adapts
the fixture into the production claim graph and inline reconciler, proving that
current-head threads suppress duplicates while stale-head threads do not. It
does not by itself prove GitHub posting or reply delivery; transactional
posting remains #748 work.

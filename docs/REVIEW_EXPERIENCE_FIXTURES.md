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

This fixture is the regression boundary for the integrated M1 review path
(#801). Its production replay adapts the fixture into the claim graph, compiler,
exact proof receipt, pending-review transport, reply delivery, and fixed-head
silence path. The fake GitHub server receives the real create/list/reconcile,
reply, head-recheck, and submit sequence; the test also verifies terminal
delivery receipts and the exact current-head bindings. It is intentionally
not an external perl-lsp run: release-installed product validation remains
owned by #806 and #808.

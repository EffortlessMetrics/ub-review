# Artifact packet

`ub-review` retains the evidence and intermediate results behind a run. Files vary with selected sensors, model modes, proof, and posting; the inventory below is the existing packet surface, not a promise that every path exists in every run.

For a first look, open `target/ub-review/running-summary.md` and `target/ub-review/review/review.md`. If a public review was prepared, inspect `review/github-review.json`; otherwise see `review/github-review-skip.json`. Posting receipts are separate from a prepared payload. The [review body contract](REVIEW_BODY_CONTRACT.md) explains what belongs on GitHub.

## Packet layout

```text
target/ub-review/
  input/
    changed-files.txt
    diff.patch
    diff-context.json

  sensors/
    tokmd/
    cargo-allow/
    ripr/
    unsafe-review/
    ast-grep/
    actionlint/
    */ub-review-sensor-status.json

  lanes/
    ub.md
    source-route.md
    tests.md
    arch.md
    opposition.md
    security.md

  candidates/
    candidate-0000-abc123def456.json
    ...

  observations/
    tests-oracle.ndjson
    source-route.ndjson
    ...

  proof_requests/
    proof-001.json
    ...

  questions/
    tests-oracle/
      red-green.json
      ...
    orchestrator-follow-up/
      follow-up-001.json
      ...

  review/
    shared_context.md
    metrics.json
    review.json
    review.md
    candidates.json
    observations.json
    unique_observations.json
    merged_observations.json
    dropped_observations.json
    orchestrator_plan.json
    final_orchestrator_plan.json
    model_stages.json
    follow_up_results.json
    follow_up_outputs.json
    follow_up_evidence.json
    resolved_candidates.json
    final_compiler_input.json
    witnesses.json
    witness_registry.json
    proof_requests.json
    proof_request_groups.json
    proof_receipts.json
    proof_plan.md
    receipt_routes.json
    resource_leases.json
    resource_plan.md
    github-review.json
    github-review-skip.json
    post-result.json
    post-error.json
    github-review-post-payload.json
    post-stdout.json
    post-stderr.txt

  events.ndjson
  candidates.ndjson
  follow_up_questions.ndjson
  follow_up_results.ndjson
  follow_up_outputs.ndjson
  resolved_candidates.ndjson
  model_stages.ndjson
  witnesses.ndjson
  proof_requests.ndjson
  proof_receipts.ndjson
  receipt_routes.ndjson
  resource_leases.ndjson
  running-summary.md
```

Start with:

```text
target/ub-review/running-summary.md
target/ub-review/lanes/tests.md
target/ub-review/lanes/ub.md
target/ub-review/input/diff.patch
```

`resolved_candidates` reconciles `review/candidates.json` with
`review/follow_up_results.json` and `review/follow_up_outputs.json`. It records
unchanged, unresolved, unavailable, resolved, or conflicting candidate state
after follow-up evidence; it is an audit receipt, not reviewer-facing text.

See [Product state](PRODUCT_STATE.md) for the distinction between prepared review, confirmed delivery, and CI result authority.

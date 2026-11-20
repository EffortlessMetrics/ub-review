# Artifact packet

`ub-review` retains the evidence and intermediate results behind a run. Files vary with
selected sensors, model modes, proof, and posting; the inventory below is the existing
packet surface, not a promise that every path exists in every run.

For a first look, open `target/ub-review/running-summary.md` and
`target/ub-review/review/review.md`. If a public review was prepared, inspect
`review/github-review.json`; otherwise see `review/github-review-skip.json`. Posting
receipts are separate from a prepared payload. The
[review body contract](REVIEW_BODY_CONTRACT.md) explains what belongs on GitHub.

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

See [Product state](PRODUCT_STATE.md) for the distinction between prepared review,
confirmed delivery, and CI result authority.

### Offline packet comparison

Run `ub-review compare-packets --stable STABLE_DIR --candidate CANDIDATE_DIR`
on two immutable directories produced by `ub-review run`. Standard output is
a deterministic `ub-review.offline_packet_comparison.v1` JSON report. The exit
status indicates whether the command produced a report. Inspect the field
statuses to see which observations are comparable.

The command validates each clean revision admission and checks the recorded
patch and changed paths against it. It compares the recorded diff context,
PR and claims inputs, thread context, the rendered shared context and its
recorded digest, effective config, plan, resolved profile,
tool plan, review options, and start-event mode. The input section records the
file hashes and options used for this comparison. Different or unavailable
inputs leave the comparison unresolved. Each file is limited to 8 MiB.

The report compares recorded proof results and command sides by receipt ID,
the legacy and split verdict fields, and cost v1 timing, cache, pricing and
token observations. Each section identifies its source path and per-side
SHA-256. Missing files, unbound legacy receipts, stale revisions and unknown
measurements remain visible. An empty proof inventory describes zero recorded
receipts. Agreement describes the reported fields only. Runtime binary
attestation, full coordinator snapshot identity, independent qualification
and promotion are outside this offline comparison.

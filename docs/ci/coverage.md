# Coverage lane

Coverage is execution-surface telemetry only. It records which Rust code paths
were exercised by the configured tests; it does not prove behavior correctness,
API conformance, parser correctness, fuzz robustness, mutation adequacy,
security posture, policy completeness, or release readiness.

## Workflow

The `Coverage` workflow runs `cargo-llvm-cov` for the Rust workspace and uploads
an `ub-review-core` flag to Codecov. The Codecov configuration is intentionally
quiet and advisory: comments are disabled, annotations are disabled, and project
and patch statuses are informational while the repository accumulates baseline
data.

The workflow emits a durable receipt at:

```text
target/coverage/coverage-receipt.json
```

That receipt is uploaded as a workflow artifact with the LCOV report, JSON
report, and text summary. It is not committed to the repository because it is a
run-specific proof artifact.

## Upload policy

- Pull requests and manual runs collect coverage as advisory telemetry.
- `main` uploads are allowed to fail CI if the Codecov upload itself fails.
- Ratchets should wait until the repository has enough baseline data to avoid
  turning telemetry into noisy theater.

## Claim boundary

This lane proves:

- the configured coverage command ran for the selected scope;
- the workflow produced coverage artifacts and a receipt when successful;
- Codecov received the scoped `ub-review-core` report when upload succeeds.

This lane does not prove:

- the tested behavior is correct;
- all important paths are exercised;
- model review lanes are adequate;
- UB sensors are complete;
- the package is ready to release.

# Coverage claims

Coverage is execution-surface telemetry. It says what code executed under a
test or workload; it does not prove behavior correctness, unsafe soundness,
mutation resistance, or release readiness by itself.

## Claim boundaries

Accepted coverage claims:

- this file, function, branch, or line executed under the named test run;
- the project or patch coverage percentage changed by the reported amount;
- an unexecuted area needs a test, runtime witness, or explicit risk decision;
- coverage was not collected, and the execution-surface evidence is unknown.

Rejected coverage claims:

- covered code is correct;
- covered unsafe code is sound;
- high coverage replaces `ripr`, `unsafe-review`, Miri, sanitizer, fuzzing, or
  runtime mutation evidence;
- a missing Codecov upload means tests did not run;
- a green coverage check is enough evidence for a release claim.

## CI posture

Coverage is not a default PR-time gate until the baseline is stable and the
branch-protection policy names the exact summary check. Before that point,
coverage should run as an informational lane on main, schedules, labels, or
selected PRs where the extra evidence is worth the cost.

Patch coverage can become blocking only after the repo has:

- stable upload authentication;
- stable path filtering;
- documented thresholds;
- a known treatment for generated files and non-Rust surfaces;
- a receipt path that records skipped, failed, and unknown coverage states.

## Tool composition

Use coverage with the rest of the Rust evidence stack:

- `ripr` answers whether changed behavior appears exposed to a meaningful
  oracle;
- `unsafe-review` answers whether unsafe/native seams have reviewable safety
  evidence;
- Miri and sanitizers answer whether selected executions hit runtime problems;
- `cargo-mutants` answers whether tests kill concrete behavioral mutants;
- Codecov reports the execution surface those witnesses touched.

Do not convert coverage telemetry into stronger product claims unless the
stronger claim is backed by the matching tool receipt.

## Receipts

Coverage receipts should identify:

- command, tool version, and feature set;
- output artifact path, such as `lcov.info` or Codecov report URL;
- upload mode and whether the upload was informational or required;
- skipped and unknown states separately from pass and fail.

Credential and rollout rules live in [Coverage](../ops/coverage.md).

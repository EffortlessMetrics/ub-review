# CI Labels

Labels are spend authorization. Use expensive labels only when the PR needs that proof now.

| Label | Meaning |
| --- | --- |
| `full-ci` | Broad validation beyond the default PR ladder |
| `ci-budget-ack` | Reviewer acknowledges an expanded default budget |
| `ci-budget-override` | Explicit hard-budget override |
| `coverage` | Coverage collection |
| `mutation` | Runtime mutation testing |
| `ripr` | Static mutation-exposure analysis |
| `ripr-waive` | Waive `ripr` with receipt-backed rationale |
| `property-tests` | Property/fuzz-style validation |
| `security-audit` | Dependency and security validation |
| `macos` / `windows` | OS-specific validation |
| `gpu-ci` | GPU/backend validation |
| `docker` | Container/image validation |
| `crossval` | External parity validation |
| `model-validation` | Model or provider validation |
| `clippy-future` | Planned stricter lint posture |

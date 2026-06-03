# CI labels

Expensive labels are spend authorization and proof requests, not convenience
toggles.

Core labels:

- `full-ci` for broad validation;
- `ci-budget-ack` for acknowledging elevated default spend;
- `ci-budget-override` for explicit hard-budget override;
- `coverage` for coverage collection;
- `mutation` for runtime mutation testing;
- `ripr` for static mutation-exposure advisory signal;
- `ripr-waive` for reviewed waivers;
- `property-tests` for property-test lanes;
- `security-audit` for dependency/security checks;
- `macos` and `windows` for OS-specific proof;
- `gpu-ci`, `docker`, `crossval`, and `model-validation` for expensive external
  validation;
- `clippy-future` for planned lint flips.

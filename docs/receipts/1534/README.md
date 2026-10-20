# Retained #1534 watchdog consumer evidence

Evidence-only branch; never merge this branch. Production repair is candidate `c0b14631cc53faae179b3c68a0049745452f442c` (tree `89169d41e134737eb9c5ab3fb3fed05355599a8c`) against `ac06293dd0871531c1826e4a3266c8dc537cf4f6`.

[evidence.tar.gz](evidence.tar.gz) contains the complete source patches, bounded fixture inputs/output artifacts, CLI harness, raw local baseline logs, native staged receipts, full RIPR reports, independent source review and runtime metadata. `watchdog-finalization-evidence/manifest.json` gives every member's size and SHA-256.

Archive: 2,353,926 bytes; SHA-256 `312c75d051d4930e520df7e99a36b1e94ce24bf92392913d1a42c8528a80b9df`.

Before repair, appended controls on unchanged production source reproduced three failures among 18 watchdog tests (15 existing passed). After repair, 19 watchdog tests and 23 actual CLI/serialized-artifact/verifier controls passed. Full workspace: 1,283 passed, one ignored; fmt/check/Clippy/doc/policy passed. Meaningful staged native check: zero blocking findings, ast-grep unavailable. Native ready RIPR analysis remains at six unsuppressed exposure gaps, not strict-zero qualification. No hosted or live watchdog proof is claimed by this archive.

Recreate the dirty before-test source by applying `before-tests.patch` to the base (resulting tree `b53e11e40ab2e2aca6857c86c0b8f8a828fef832`); `repair.patch` reconstructs the final repair tree from the same base. Rust 1.95.0, Python 3.12.14, Linux x86_64. Exact commands and timing are retained in `runtime.json`, `validation.json` and the harness source.

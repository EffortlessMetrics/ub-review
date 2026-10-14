# UB-review #1460 native evidence

Candidate: 46bfbd5e2a1f8d2a78d2aaecfaffcf7df78da564
Tree: 410b67ab138bc8f5075ea9b7e71c3081831560a7
Sole parent/base: 3dbcc2be9768b082c0896dc2360e950ffbd56a65

Rust 1.95.0 on Linux. Full native chain passed fmt, check, 1,270 top-level tests plus one nested pass/one intentionally ignored driver, strict Clippy, docs and policy. The process session ended during its final precommit; terminal precommit was independently rerun and is recorded in precommit-final.log with exit 0 and zero blocking findings. Existing rustdoc/policy warnings and missing analyzers are retained. The artifact-verifier self-test used unchanged verifier source and skips the Windows junction case on Linux.

Independent exact-commit review verified 19 capture identities, the source projections and fixed scope. All 11 grouped tests passed. Three previously accepted manifest-only falsifiers now fail at their intended semantic joins; mutation receipts retain separate experiment trees and restored candidate identity. This verifies the bounded fixed fixture only, not a generic runtime verifier or complete #1270 acceptance.

A subsequent separately installed public RIPR 0.10.0 run completed in advisory mode with 203 unsuppressed gaps. That is not strict-zero clearance. The source branch stays Draft pending finding disposition and other qualification. No suppression/threshold/pin changes were made. The 287 MB detail stream remains local; only bounded aggregate provenance is retained here. Native checks, independent review, hosted required checks, analyzer disposition and merge eligibility are separate.

These files are evidence only, not candidate source changes. No savings, receipt equivalence, complete corpus acceptance or release claim is made.

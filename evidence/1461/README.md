# Native validation for UB-review #1461

Source head: e5fc48da60bc1b887cfdc4efc2ee28d942a0a279
Base: 3dbcc2be9768b082c0896dc2360e950ffbd56a65
Tree: cc08312e4c857994a153a537082f6776b0fc54a7

The independently reviewed local commit f7c54ca8beb1c36b0267d5376e3be347b3ee945b has identical tree and sole parent; the final review records the independently checked published-head bridge. These receipts establish bounded reader-only native validation and six compiled negative controls, not full reconciliation parity, hosted gate success or merge qualification.

Author baseline: 1,298 workspace tests passed, plus unchanged Python 51-test corpus; fmt/check/strict Clippy/docs/policy/artifact verifier terminal. One intentional subprocess-driver ignore; inherited rustdoc and overdue-policy warnings; Windows junction check skipped. Precommit is advisory and capture-truncated for RIPR, with missing cargo-allow, unsafe-review and ast-grep.

Independent review: 71 xtask units, one smoke CLI and ten reader/parity CLI tests; six compiled mutants rejected, original bytes restored and positives rerun. Early report documents defects corrected before the frozen candidate.

Original logs may contain local paths and timings. They are retained execution context, not portable authority. Follow the exact source identities and SHA256SUMS.

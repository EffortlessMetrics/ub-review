# Early independent review: #1292 bounded packet reader

Scope: reusable bounded reader and internal xtask shadow inspection only. No reducer, ledger replay, coherent verdict, gate switch, publication or cloud session. This is not exact-final-head clearance.

Author checkout base: 3dbcc2be9768b082c0896dc2360e950ffbd56a65. Author files were never edited. All execution used the self-copied isolated lab `review-projection-reader-early` with its own `review-projection-reader-target` Cargo target directory. Toolchain: cargo/rustc 1.95.0.

## Reviewed stable early snapshot identities

- reader.rs: 6e3869bc4a04fd37ce9a8e3d153d8984475894ddc9ec3843e26eca198770771e
- reader/tests.rs: f3ef5cb47d2c2b3aa05cb843e8edd469be391debf1af9d627f6db96e3085b3e8
- xtask/src/task_projection_reader.rs: 68c2e1222c191b6b1aa3d4577a2dd5a24b8b2f34d07f149c4c9b6d57294b7e77
- xtask/tests/task_projection_reader_parity.rs: 7d69d8da611e939936f36c4b2d9395e563f4b6692edb4872c4737161b72a4c8e
- xtask/Cargo.toml: 6e51a32e3ca80f0435ea5953f7101286f263ee27225a0426862678bbcc1099b0

## Actionable findings and resolution

1. Package-only xtask build failed because serde lacked the derive feature while Source/Issue derived Serialize. Workspace root serde/derive could mask it. Author enabled derive explicitly. Stable package-only xtask tests pass.
2. New serde_json arbitrary_precision/raw_value features changed existing root generic Value parsing under workspace builds. Number magic-key objects became numbers, RawValue magic-key objects became decoded values, nonfinite exponents were accepted, and some valid objects became parser errors. Three reviewer-owned root sentinels passed under package-only selection and all failed under workspace selection before the fix. Author replaced packet input with a reader-owned value/raw-number representation and feature-free validation. Both root selections now pass all three, and workspace feature tree contains only default/std.
3. Legal control-byte sensor names could inflate escaped source metadata above 512 KiB within the ordinary file/byte budgets. The command returned exit 1 and empty stdout. Author now emits a tiny versioned unavailable reader receipt and exit 2. Unchanged independent reproducer now returns 343 bytes, exit 2, empty stderr.
4. Rejected oversized proof/lease arrays were still rescanned and proof references collected after bounded row validation. Author removed that rescan/allocation and added parser-wide container/node limits. No false-success bug was established for the earlier version.
5. Finite-f64 validation can reject oversized integer literals in unused fields. Author now documents this alongside container/node/Unicode differences; full u128 remains preserved and tested.

## Independent execution

- `cargo test --locked --offline -p xtask`: 71 unit tests, 1 smoke CLI test, 10 reader/parity CLI tests passed. This includes 29 reader unit tests. Log: review-projection-native-snapshot-tests.log.
- `python3 review-projection-parser-corpus.py`: all 58 unchanged actual worker-command cases passed before and after native redesign. Cases cover u128 and overflow, signed zero, exponent/fraction domains, magic keys, escaped Unicode duplicates, odd/even backslash string boundaries, malformed JSON, invalid UTF-8, illegal whitespace and excess depth.
- `python3 review-projection-report-budget.py`: pre-fix exit 1/no JSON; fixed snapshot exit 2/343-byte JSON/empty stderr.
- Reviewer-only `tests/serde_json_ambient_contract.rs` in copied lab: three tests pass under both `cargo test --locked --offline -p ub-review --test serde_json_ambient_contract` and `cargo test --locked --offline --workspace --test serde_json_ambient_contract` after feature removal. Not proposed source changes.
- Before-fix root package/workspace logs: review-projection-ambient-package.log and review-projection-ambient-workspace-before.log.
- After-fix root package/workspace logs: review-projection-ambient-package-after.log and review-projection-ambient-workspace-after.log.

No remaining material parser/path/cache/UTF-8/u128 finding identified on these snapshot bytes. Fresh frozen candidate review remains required. No Python reducer parity, full production regression, hosted checks, independent exact-final-head approval, Ready or merge qualification is established by this review.

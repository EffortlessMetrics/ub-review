# Native reader metadata-error regression control (#1461)

A bounded missing test oracle was found; the observed production behavior is correct. Published source head `557d7f82e2584c85f7b8c25ba6ca1aaf3ca45a43`, tree `c6bf736b53e6bd4dca2a1cc0fcb3a854800a3c9c`, sole parent `e5fc48da60bc1b887cfdc4efc2ee28d942a0a279`. The independently reviewed local commit `6549a9fed193bba0b5857a1d7f3b776d743a9c8c` has identical tree/parent. Only reader/tests.rs changed: 26 added lines, one Linux-gated test. Production, policy, config and dependencies are unchanged.

## Evidence

Both implementation validation and fresh independent review used Rust/Cargo 1.95 from repository cwd. The original 82 xtask tests survive a compiled mutation changing confined_path's generic metadata error from InputIo to UnsafePath. The new test kills that mutant and error-arm fallthrough with assertion failure, exit 101. Independent review additionally killed an InputIo-specific failed-cache bypass at the zero-budget revisit assertion. Controls 2 and 3 make no old-suite-survival claim.

Final candidate passes 72 unit + 1 smoke CLI + 10 reader CLI tests (83 total), fmt, strict xtask all-target Clippy, and policy check. Independent review separately passed 83 tests, fmt and Clippy, then restored pristine source and reran all 30 reader tests after its controls. Author receipts retain preliminary Rust1.99 observations separately; final authoritative controls and validation used1.95.

The test uses a fresh temporary directory and a 65,536-byte ASCII path component, which the Linux host rejects with ENAMETOOLONG without chmod, root assumptions, special files or races. It directly asserts confined_path's InputIo before any read attempt; then asserts cached InputIo on a required JSON revisit after the attempt budget becomes zero, one attempt, zero bytes, no sources, invalid coverage, unavailable input and one invalid_projection diagnostic.

## Boundaries

This establishes the Linux metadata-error mapping and cache accounting only. It does not establish other File::open/read_to_end error origins, Windows/macOS behavior, race-resistant packet custody, or full reconciliation. No production failure was observed or fixed. No blanket test-adequacy or mutation-clearance claim follows.

Prior native source evidence remains at https://github.com/EffortlessMetrics/ub-review/tree/bc92afafbd615664a562331e08833a49c8e5d86e/evidence/1461 . This follow-up has fresh delta review and native execution; the previous hosted FAIL313/max0 and analyzer comparison remain bound to e5fc48da. New-head hosted checks must be established separately. No Ready/merge/adoption clearance is asserted.

## Contents

Author and independent-review logs, patches and receipts are prefixed separately. Absolute paths in raw execution logs identify isolated experimental working directories; they are not public artifact links. Only explicitly bundled files are available here. SHA256SUMS covers all files except itself. No binaries or temporary Git indexes are included.

# Bun UB review posture

Use this packet as evidence, not as a verdict. Do not infer safety from missing
sensor output. Missing evidence is a review question, not a clean result.

Standalone approval language is banned. Do not use a one-word approval, a
generic quality adjective, or a zero-actionable shorthand as the conclusion.

A no-finding lane must provide:

1. what concrete paths, invariants, tests, or claims were checked;
2. the strongest failed objection;
3. why that objection did not hold;
4. residual risk for a human to verify.

Bun-specific high-risk seams:

- ArrayBuffer / TypedArray resize, detach, transfer, or GC
- active view region vs whole backing store
- JS-backed memory handed to async workers
- JSC protect/unprotect lifetime balance
- Rust/Zig/C++ FFI ownership and allocator mistakes
- string/buffer helper routes shared by crypto, compression, and runtime APIs
- tests that only document behavior rather than proving red/green movement

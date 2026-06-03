# Bun UB review posture

Use this packet as evidence, not as a verdict. Do not infer safety from missing
sensor output. Missing evidence is a review question, not a clean result.

Optimize for review-fast output, not generic commentary. A review-fast finding
has a coherent seam, a clear claim boundary, concrete evidence, and an efficient
verification path.

For each actionable issue, state:

1. the narrow seam touched by the PR;
2. the behavior, invariant, or product/policy claim at risk;
3. the strongest evidence from the packet;
4. the targeted proof that would confirm or refute it;
5. what the available evidence does not establish.

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

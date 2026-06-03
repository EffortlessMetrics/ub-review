# CI Lane Map

Every CI lane must answer:

- What failure mode does this catch?
- Why does it need to run on ordinary PRs?
- What is its estimated LEM?
- What cheaper signal was considered first?
- What artifact proves it ran?
- What makes it skip safely?

Lane definitions live in `policy/ci-lanes.toml`; routed bundles live in `policy/ci-risk-packs.toml`.

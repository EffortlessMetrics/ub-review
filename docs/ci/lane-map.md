# CI lane map

Every CI lane needs named intent. A lane should answer:

- What failure mode does this catch?
- Why does it need to run on ordinary PRs?
- What is its estimated LEM?
- What cheaper signal was considered first?
- What artifact proves it ran?
- What makes it skip safely?

Lane definitions live in `policy/ci-lanes.toml`; risk-pack routing lives in
`policy/ci-risk-packs.toml`.

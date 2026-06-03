# Branch Protection

Branch protection should prefer one stable aggregate required check over many
required leaf jobs.

Recommended required check:

```text
PR Gate Success
```

Current workflows still expose leaf jobs directly. Until a summary gate exists,
`policy/ci-lanes.toml` is the source of truth for which jobs are intended to be
blocking and which are advisory, path-gated, label-gated, or release-only.

## Rules

- Leaf jobs may be skipped by policy.
- Skipped optional lanes must be explained.
- Matrix leaves should not become branch-protection contracts.
- Advisory jobs must not silently become blocking.
- Path-filtered workflows should not leave required checks pending forever.

The planned `workflow_policy` lane should lint workflow jobs against the lane
registry before branch protection is tightened.

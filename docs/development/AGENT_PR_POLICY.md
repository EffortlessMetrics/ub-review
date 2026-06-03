# Agent PR policy

Agents should work PR-by-PR, not as mega-PR generators.

Use this sequence:

1. Inspect current repo state.
2. Make a scoped change.
3. Run acceptance checks.
4. Open with purpose, risk, and rollback notes.
5. Address actionable bot comments and CI failures before moving on.
6. Merge when green, then start the next PR.

Use separate PRs unless a stacked sequence is explicitly requested. Do not make a
mega-PR. If planned work already exists, convert the PR into audit, cleanup, or
doc-sync work instead of silently changing scope.

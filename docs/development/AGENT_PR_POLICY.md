# Agent PR Policy

Agents must work PR-by-PR, not mega-PR-by-mega-PR.

## Required workflow

1. Inspect current repository state.
2. Make a scoped PR.
3. Run acceptance checks.
4. Open with purpose, risk, rollback, and test summary.
5. Address actionable bot comments and CI failures before moving on.
6. Merge only when green and reviewed.

Use separate PRs unless a stacked sequence is explicitly requested. Do not make a mega-PR. If planned work already exists, convert the PR into audit, cleanup, or doc-sync. Address bot comments and CI failures before moving on.

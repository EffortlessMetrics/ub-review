# UB ledger integration

Keep the ledger outside all Bun worktrees.

Recommended path:

```text
/home/steven/code/bun-ub-ledger/
```

Recommended structure:

```text
README.md
ub-ledger.yaml
lanes.yaml
pr-index.yaml
artifacts/
scripts/
```

Durable campaign memory should be updated by a single summary reducer or human-reviewed script, not by every lane.

`ub-review run` reads the configured `repo.ledger` path as bounded shared
context when it exists. It does not mutate the ledger. A missing ledger is
recorded as unavailable context rather than treated as a failure.

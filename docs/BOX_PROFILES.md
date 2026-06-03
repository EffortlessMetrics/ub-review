# Box profiles

Profiles let the same review intent run on different hardware safely.
Use `--runtime-profile` to choose these box budgets explicitly. The older
`--profile` option remains a compatibility alias for the same runtime profiles.

| Profile | Local posture |
|---|---|
| `gh-runner` | ephemeral, disk-constrained, artifact-oriented |
| `cx23` | tiny coordinator, high remote thinking, minimal local work |
| `cx33` | balanced small box, full fast sensors |
| `cx43` | stronger local sensor box, occasional tests/builds |
| `auto` | conservative detection |
| `custom` | config-owned |

Review breadth and local work are separate.

```text
20 logical lanes can be fine.
20 local tool monsters are not fine.
```

Runtime profiles now set model fanout, sensor worker fanout, and focused proof
budgets. The effective values are emitted in `resolved-profile.json` and
`resolved-plan.json` under `limits.*` and `budgets.proof_*`.

If a guard fails, sensors degrade and the summary records missing evidence.

# Box profiles

Profiles let the same review intent run on different hardware safely.

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

If a guard fails, sensors degrade and the summary records missing evidence.

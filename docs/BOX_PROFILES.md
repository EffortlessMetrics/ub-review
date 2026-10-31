# Box profiles

The Bun review profile lives in `profiles/bun-ub-v0.toml`. Runtime profiles are
documented in `docs/RUNTIME_PROFILES.md` and encoded as technical presets under
`runtime/*.toml`: `gh-runner`, `gh-runner-standard`, `gh-runner-full`, `cx23`,
`cx33`, and `cx43`.

Use `--runtime-profile` for box budgets explicitly, while the older `--profile`
option remains a compatibility alias for those runtime budgets. The action's
default `profile: gh-runner` is the zero-config standard GitHub-runner lease;
set `runtime-profile: gh-runner-full` only when the repo is intentionally
leasing broader proof.

| Profile | Local posture |
|---|---|
| `gh-runner` | zero-config alias for `gh-runner-standard` |
| `gh-runner-standard` | GitHub-hosted runner, focused proof, artifact-oriented |
| `gh-runner-full` | explicitly leased GitHub runner, broader proof and leased heavy witnesses |
| `cx23` | tiny coordinator, high remote thinking, minimal local work |
| `cx33` | balanced small box, full fast sensors |
| `cx43` | stronger local sensor box, occasional tests/builds and leased heavy witnesses |
| `auto` | conservative detection |
| `custom` | config-owned |

## Generating a starter

`init --profile` generates the built-in profiles above or `auto`.
`init --profile custom` is rejected before creating or overwriting either the
configuration or the guide, including with `--force` or `--no-guide`. The shared
profile argument still lists `custom` because runtime consumers support it;
`init` cannot invent the repository's custom budgets.

To use a custom profile, generate a built-in starter, define `[profiles.custom]`
with the intended settings, and select `profile = "custom"`. The table key is
the runtime profile's identity; an omitted or different `name` inside the table
does not change that identity. Commands that read that configuration, including
`run`, `plan`, and `doctor`, retain custom-profile support. The rejection applies only to generating an undefined custom starter.

Review breadth and local work are separate.

```text
20 logical lanes can be fine.
20 local tool monsters are not fine.
```

Runtime profiles set model fanout, sensor worker fanout, and focused proof budgets. The effective values are emitted in `resolved-profile.json` and `resolved-plan.json` under `selected_review_profile`, `selected_runtime_profile`, `limits.*`, and `budgets.proof_*`.

If a guard fails, sensors degrade and the summary records missing evidence.

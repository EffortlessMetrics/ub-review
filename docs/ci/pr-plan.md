# PR plan

The durable CI control plane should live in repo-native code, not in ad hoc YAML
or scripts. The target interface is:

```bash
cargo xtask ci plan
```

The command should write `target/ci/ci-plan.json` with schema version, changed
files, affected areas/crates, selected risk packs, selected lanes, budget band,
estimated LEM, limits, and warnings.

A minimal plan shape is:

```json
{
  "schema_version": 1,
  "budget": {
    "estimated_lem": 34,
    "band": "default",
    "default_limit_lem": 35,
    "hard_limit_lem": 125
  },
  "changed": {
    "files": [],
    "areas": [],
    "crates": []
  },
  "selection": {
    "risk_packs": [],
    "lanes": []
  },
  "warnings": []
}
```

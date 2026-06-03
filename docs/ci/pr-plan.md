# PR CI Plan

The durable planner shape is Rust-native and testable:

```text
changed files + labels + cargo graph + historical timing
        ↓
xtask ci plan
        ↓
ci-plan.json
        ↓
selected lanes + estimated LEM + risk packs
        ↓
CI actuals
        ↓
learned estimates
```

`target/ci/ci-plan.json` should use this schema shape:

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

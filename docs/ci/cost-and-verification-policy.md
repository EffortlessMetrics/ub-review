# Cost and verification policy

CI cost discipline exists so `ub-review` can afford more verification, not less.
Agentic development raises verification demand: at industrialized PR volume,
small per-PR costs become material and verification cost can exceed LLM cost.
The answer is verification architecture.

The model is:

```text
Suggestions → Assisted → Native → Industrialized
```

As work moves toward industrialized throughput, the repo needs scoped,
deterministic, Rust-native proof that is cheap enough to run continuously.
Default PR lanes should be sharp autofocus. Main, nightly, release, and labeled
lanes should preserve the high-resolution scan.

## PR-time posture

Default PRs should run:

- PR plan;
- Rust fast gate;
- policy gate;
- `ripr` advisory/static exposure where available;
- selected risk-pack checks;
- one summary check named `PR Gate Success`.

Deep lanes such as coverage, runtime mutation, fuzzing, full feature matrices,
Docker, platform runners, and release dry-runs remain strong but are label,
schedule, main, or release gated.

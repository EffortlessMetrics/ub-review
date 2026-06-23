# Cost and verification policy

CI cost discipline exists so `ub-review` can afford more verification, not less.
Agentic development raises verification demand: at industrialized PR volume,
small per-PR costs become material and verification cost can exceed LLM cost.
The answer is verification architecture.

The model is:

```text
Suggestions -> Assisted -> Native -> Industrialized
```

As work moves toward industrialized throughput, the repo needs scoped,
deterministic, Rust-native proof that is cheap enough to run continuously.
Default PR lanes should be sharp autofocus. Main, nightly, release, and labeled
lanes should preserve the high-resolution scan.

> **Current enforcement state (#601):** the label-gating model described below
> is documented but **not yet wired into the workflow**. The actual posture is
> that `ub-review/gate` runs with `allow-heavy: 'true'` on every PR push (no
> label gate). The LEM budget bands, risk packs, and label-selectable deep
> lanes in `policy/ci-{budget,lanes,risk-packs}.toml` are seed contracts.
> Wiring label-gating is a future product decision; until then, this document
> describes the target posture, not the enforced one.

## PR-time posture

The target default PR posture is:

- PR plan;
- work queue with packet deadlines and late receipt routing;
- Rust fast gate;
- policy ledger parse/check once implemented;
- `ripr` advisory/static exposure where available;
- selected risk-pack checks;
- one summary check named `PR Gate Success`.

Deep lanes such as coverage, runtime mutation, fuzzing, full feature matrices,
Docker, platform runners, and release dry-runs remain strong but are label,
schedule, main, or release gated.

# CI Policy Rollout Plan

Roll out CI economics in reversible steps:

1. Add doctrine docs and policy ledgers.
2. Inventory existing workflows and map jobs to lanes.
3. Keep policy advisory while actuals are absent.
4. Add workflow-policy linting against the lane registry.
5. Add a stable `PR Gate Success` summary gate.
6. Emit `target/ci/ci-plan.json` for forecasted lanes and LEM.
7. Emit `target/ci/ci-actuals.json` for measured duration and cache behavior.
8. Add cache and concurrency policy receipts.
9. Add `ripr` advisory output and suppression workflow.
10. Compare learned estimates against static estimates.
11. Warn on elevated/default overages.
12. Enforce hard ceiling only with override labels.

No branch-protection gamble should happen before skipped optional lanes, advisory
lanes, and aggregate gate behavior are observable.

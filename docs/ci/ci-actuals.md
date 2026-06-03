# CI Actuals

CI actuals are timing and outcome receipts emitted by lanes after execution. They let the planner learn from real cost instead of guessing from static workflow definitions.

Each lane should write `target/ci/ci-actuals-<lane>.json` when timing data is available. Actuals should include lane name, runner, start/end timestamps, elapsed seconds, estimated LEM, observed LEM, cache hit state, selected risk packs, and artifact paths.

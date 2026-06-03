# CI actuals

Each CI lane should record an actuals artifact such as
`target/ci/ci-actuals-<lane>.json`. Actuals let the planner compare estimates
against real runtime and adjust future lane selection.

Actuals should include lane name, runner, start/end timestamps, elapsed seconds,
normalized LEM, success/failure state, selected risk packs, and output artifact
paths.

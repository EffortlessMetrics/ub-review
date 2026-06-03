# LEM budgeting

A Linux-equivalent minute (LEM) is the normalized CI budget unit used to compare
lanes with different runner costs. The planner should estimate LEM before a run
and record actuals afterward.

`policy/ci-budget.toml` defines the default and hard budget bands. The intended
flow is:

```text
changed files + labels + cargo graph + historical timing
        -> xtask ci plan
        -> target/ci/ci-plan.json
        -> selected lanes + estimated LEM + risk packs
        -> CI actuals
        -> learned estimates
```

Budget overrides should be explicit through labels such as `ci-budget-ack` and
`ci-budget-override`.

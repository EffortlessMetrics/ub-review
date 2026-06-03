# LEM Budgeting

LEM means Linux-equivalent minutes:

```text
LEM = wall-clock minutes × runner multiplier
```

A 10-minute Linux job, a 10-minute Windows job, and a 10-minute macOS job are
not the same economic object. The normalized budget lives in
`policy/ci-budget.toml`.

## Bands

| Band | LEM | Meaning |
| --- | ---: | --- |
| Pennies | 0-12 | docs, metadata, light checks |
| Default | 13-35 | ordinary Rust PR |
| Elevated | 36-75 | risk-expanded PR |
| High | 76-125 | explicit expensive PR |
| Over ceiling | >125 | requires override |

Ordinary Rust PR target:

- preferred default: below 25 LEM;
- default limit: under 35 LEM;
- preferred dollar posture: below roughly $0.50 at the Linux minute rate;
- hard ceiling posture: about $1 unless explicitly overridden.

## Spend authorization

Labels are spend authorization, not convenience toggles. A label such as
`full-ci`, `ub-review-model-smoke`, or `release-check` means the reviewer is
buying additional proof beyond the ordinary PR path.

## Receipts before enforcement

The planner should eventually write `target/ci/ci-plan.json` and CI should emit
`target/ci/ci-actuals.json`. Enforcement should compare estimates against
actuals, cache behavior, queue time, failure rate, flake rate, and MTTR before
turning advisory limits into hard gates.

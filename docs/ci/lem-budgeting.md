# LEM budgeting

LEM is Linux-equivalent minutes. The planning loop is:

```text
changed files + labels + cargo graph + historical timing
-> ci-plan.json
-> selected lanes
-> CI actuals
-> learned estimates
-> budget warnings / guardrails
```

The standard budget bands are encoded in `policy/ci-budget.toml`:

| Band | LEM | Meaning |
| --- | ---: | --- |
| pennies | 0-12 | docs, metadata, very light checks |
| default | 13-35 | ordinary Rust PR |
| elevated | 36-75 | scoped high-risk PR |
| high | 76-125 | explicit expensive PR |
| over ceiling | >125 | override required |

Ordinary PRs should stay in the default band unless the PR carries an explicit
risk label or policy reason. This is not a reason to skip important proof; it is
a reason to select the proof that is most likely to catch the changed risk.

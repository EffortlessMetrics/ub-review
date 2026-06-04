# ub-review

ub-review is a targeted CI runner with review judgment automation built in.

Sharper: ub-review is an intelligent PR CI gate. It decides what evidence a PR needs, runs the relevant proof, and turns the result into one concise review decision.

It turns one CI runner into an investigation bench: checkout once, diff once, plan evidence from the PR shape, run on-diff sensors once, start read-only model lanes immediately, run relevant proof centrally, and post one concise PR Review.

The goal is not the fastest possible review. The goal is the best useful review inside the runner budget.

## Operating principle

Whole-runner stewardship: while the runner is live, every useful resource serves the review.

CPU runs focused tests. Disk holds proof worktrees. Memory holds evidence packets. Model budget goes to reasoning over prepared evidence. Remote model calls run concurrently with local proof; provider wait does not occupy the runner's local compute lease. Time is spent producing receipts.

## Product sentence

ub-review prepares evidence, reasons about it, proves what it can, and
reports only what changes the reviewer's decision.

## Review contract

The PR body contains only reviewer-value content:

- findings
- verification questions
- proof results
- refutations
- parked follow-ups
- specific evidence gaps that affect trust

Everything else goes to artifacts:

- lane outputs
- model status
- sensor logs
- proof stdout/stderr
- resource leases
- metrics
- raw observations

The default `[review_body]` policy enforces that split:

```toml
[review_body]
include_successful_lane_table = false
include_provider_table = "on_failure"
include_sensor_table = "on_failure"
include_execution_summary = "none"
```

The concrete PR-commentary style contract is
[REVIEW_BODY_CONTRACT.md](REVIEW_BODY_CONTRACT.md).

## Execution contract

Models investigate. Tools produce receipts. The compiler decides what earns the
reviewer's time.

## External positioning

ub-review is an evidence-first CI gate for review-fast PRs. It uses the runner as an investigation bench: prepare context once, run targeted proof, let focused model lanes reason over receipts, and post only the decision-relevant result.

## Gate category

`ub-review` is not a generic LLM comment bot, and it is not fixed-job CI. It is
an intelligent PR CI gate:

```text
PR diff -> targeted evidence plan -> sensors/tools/tests -> model lanes + proof broker -> proof receipts -> decision memo
```

The runner prepares evidence once, leases local proof work while remote model
calls wait on network I/O, and compiles one review artifact. A useful pass is
not measured by how much text it posts or whether a fixed job matrix passed; it
is measured by whether it proves what this PR needed proven and changes a
reviewer's decision with grounded evidence.

## Three-stream architecture

The runner treats a PR pass as three overlapping streams:

```text
evidence stream:
  diff, line map, PR thread, tokmd, ripr, unsafe-review, ast-grep, cargo-allow, coverage

model stream:
  cached shared packet, routed lanes, proof-planner, follow-ups, refuter

proof stream:
  affected checks, focused tests, base+tests red/green, actionlint, coverage, targeted mutation/sanitizer when leased
```

The scheduler records stream lifecycle and timing separately so remote model
wait does not masquerade as consumed local proof time. The product value comes
from overlap: models investigate while tools produce receipts and the compiler
turns only decision-relevant evidence into review output.

## Judgment layer

Review judgment automation means the system decides:

- which lanes are relevant;
- which tools should run;
- which proof is worth the runner time;
- which findings are real;
- which gaps matter;
- which comments earn reviewer time;
- when the pass is sufficient.

It does not mean the model's opinion becomes truth. Models investigate, tools
produce receipts, and the compiler decides what gets posted.

## Evidence stack

The Rust-first stack is deliberately layered:

- `tokmd` prepares deterministic repository, diff, cockpit, and bounded-context
  packets.
- `cargo-allow` owns retained source-tree exceptions.
- `ripr` owns static mutation-exposure and weak-oracle signal.
- `unsafe-review` owns unsafe/native reviewability.
- `ast-grep` and actionlint own structural sibling and workflow checks.
- Codecov, cargo-mutants, Miri, and sanitizers are scoped runtime backstops.

`ub-review` calls those tools, normalizes receipts, routes evidence, and files
grounded defects in the matching tool repo. It does not reimplement specialized
tool logic, and it does not turn missing evidence into a pass.

## Intelligent PR gate

The review compiler is the product boundary. It should post only evidence that
changes reviewer action:

- confirmed findings;
- verification questions;
- proof results and refutations;
- residual risk;
- trust-affecting missing evidence.

Successful setup tables, lane rosters, provider status, raw command logs,
generic no-finding prose, and scratch observations stay in artifacts. A clean
run can produce no PR review payload; a degraded run should name the missing or
failed evidence that changes trust.

## Economics

The scarce resource is the runner lease. Trusted repos default to two useful
passes, `opened` and `ready_for_review`, with no full `synchronize` spend unless
the repo opts in. Each standard pass targets 30 minutes of local proof work and
has a 60-minute hard timeout.

The proof planner and proof broker keep that economics honest: the runner
chooses work like a reviewer, lanes request proof, each useful command executes
once, and proof runs only when it is likely to change the review decision.

## Dogfood boundary

The Bun UB profile is the first hard calibration target, not the default shape
of every repository. It stresses the system on native-boundary review problems:

- resizable ArrayBuffer active-view and backing-store mistakes;
- stale pointer/length hazards;
- detach, transfer, worker handoff, and GC lifetime risks;
- tests that execute code but fail to prove the changed behavior;
- unsafe/native contracts that need explicit review evidence.

Bun findings should be recorded as calibration evidence only at the strength the
receipts support. Draft or unmerged findings are useful signal, not stronger
product proof.

## Proof obligations

1. Keep PR review bodies free of boilerplate in clean and degraded runs.
2. Expand proof-broker and resource-broker coverage from focused v0 proof to
   risk-routed runtime backstops.
3. Keep runner images aligned with the documented sensor stack.
4. Calibrate Bun UB findings through the ledger without overstating unmerged
   evidence.
5. Preserve claim traceability from every review statement back to diff, sensor,
   model lane, or proof receipt.
6. Keep the internal north star true: the runner does the work traditional CI
   would do, but chooses it like a reviewer.

# ub-review

ub-review is an evidence-first CI review gate.

It turns one CI runner into a review cockpit: checkout once, diff once, run on-diff sensors once, start read-only model lanes immediately, run relevant proof centrally, and post one concise PR Review.

The goal is not the fastest possible review. The goal is the best useful review inside the runner budget.

## Operating principle

Whole-runner stewardship: while the runner is live, every useful resource serves the review.

CPU runs focused tests. Disk holds proof worktrees. Memory holds evidence packets. Model budget goes to reasoning over prepared evidence. Remote model calls run concurrently with local proof; provider wait does not occupy the runner's local compute lease. Time is spent producing receipts.

## Product sentence

ub-review prepares evidence, runs focused investigation lanes, proves what it
can, and reports only what changes the reviewer's decision.

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

## Execution contract

Models investigate. Tools produce receipts. The compiler decides what earns the
reviewer's time.

## External positioning

ub-review is proof-backed PR review. It uses the CI runner as an investigation bench: prepare evidence once, run focused model lanes, execute relevant proof centrally, and post only the result a reviewer needs.

## Gate category

`ub-review` is not a generic LLM comment bot. It is an evidence-first CI review
gate:

```text
diff packet -> sensors -> model lanes + proof planner/proof broker -> review compiler
```

The runner prepares evidence once, leases local proof work while remote model
calls wait on network I/O, and compiles one review artifact. A useful pass is
not measured by how much text it posts; it is measured by whether it changes a
reviewer's decision with grounded evidence.

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

The proof planner and proof broker keep that economics honest: lanes request
proof, the runner executes each useful command once, and proof runs only when it
is likely to change the review decision.

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

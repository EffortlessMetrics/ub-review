# ADR 0001: Whole-runner stewardship

## Status

Accepted

## Context

ub-review is an evidence-first CI review gate. It uses the whole runner to turn a PR into proof-backed review feedback.

CI review has a short-lived but powerful execution environment: CPU, disk, memory, I/O, model budget, and wall-clock time are all available only while the runner is live. Spending those resources on repeated checkout, duplicated repo navigation, disconnected model work, or verbose posting reduces the evidence available to the human reviewer.

The review gate therefore needs a clear resource rule and plain implementation
names. Runtime profile names are technical: `gh-runner-standard`,
`gh-runner-full`, `cx23`, `cx33`, and `cx43`. The default action input
`gh-runner` remains a compatibility alias for the standard GitHub-runner lease.

## Decision

The runner exists to serve the review.

The orchestrator may spend CPU, memory, disk, I/O, and model budget when the work is:

- relevant to the PR;
- centrally scheduled;
- deduped;
- budgeted;
- leased;
- receipted;
- likely to change the review decision.

Whole-runner stewardship is the operating principle: while the runner is live, every useful resource serves the review.

```text
CPU       runs focused tests and lightweight proof
disk      holds base+tests worktrees and receipts
memory    keeps packets, observations, and sensor output available
models    reason over prepared evidence, not repo navigation
time      is spent producing proof, not posting boilerplate
```

ub-review prepares evidence, runs focused investigation lanes, proves what it
can, and reports only what changes the reviewer's decision.

## Architecture rule

Lanes request proof.
The orchestrator ranks and routes proof requests.
The proof broker runs commands.
The resource broker enforces budgets and leases.
The compiler posts one concise review.
Artifacts preserve the full audit trail.

The review compiler may include only reviewer-value content in the PR body:

- findings;
- verification questions;
- proof results;
- refutations;
- parked follow-ups;
- specific evidence gaps that affect trust.

Setup dumps, lane rosters, generic warnings, status tables, command logs, model status, raw observations, proof stdout/stderr, metrics, and resource leases belong in artifacts.

## Runtime defaults for trusted repositories

Trusted-repository defaults are two passes per PR:

- `opened`;
- `ready_for_review`.

There is no default `synchronize` trigger. A new commit should not automatically spend another full runner unless the repo explicitly opts into that cost.

Each pass targets 30 minutes and has a hard timeout of 60 minutes. The standard pass emphasizes focused tests, base+tests red/green, actionlint, scoped source-route checks, and other lightweight proof. Targeted mutation and sanitizer witnesses run only when a runtime profile leases them.

## Consequences

- Models investigate prepared evidence instead of rediscovering the repository.
- Proof-producing tools are centralized so the same command is not run by multiple lanes.
- Resource use is auditable because every significant spend is leased and receipted.
- The PR review remains concise because artifacts carry the audit trail.
- Runtime profile names stay operational and technical rather than branded or poetic.

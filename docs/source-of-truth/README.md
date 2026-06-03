# Source-of-truth stack

This repo is operated by contract.

Proposals explain why work exists. Specs define required behavior and evidence.
ADRs record durable architecture decisions. Plans break accepted work into
PR-sized slices. Active goal manifests tell agents what is being executed now.
Support tiers map user-facing claims to proof commands. Policy ledgers record
governed exceptions and CI, lint, package, and file rules. CI validates the
contracts. Closeouts record what landed, what proved it, what remains, and what
should happen next.

Work one PR-sized slice at a time. Verify every named command, workflow, lint,
schema field, crate, path, feature, and policy before relying on it. Treat the
current repo state, GitHub PRs/issues, policy ledgers, active manifests, CI
state, and explicit user direction as the live operating board. Improve aligned
agent PRs when correct and green; split, defer, or close only with precise
evidence. Leave the repo cleaner than before: proof recorded, claims bounded,
ledgers current, no stale artifacts, and no ambiguous open work.

## Artifact responsibilities

| Artifact | Owns | Does not own |
| --- | --- | --- |
| Proposal | Why the work exists, the problem, alternatives, and success criteria. | Required runtime behavior or PR sequencing. |
| Spec | Required behavior, evidence, acceptance examples, non-goals, and failure modes. | PR order or roadmap batching. |
| ADR | Durable architecture or policy decisions and their consequences. | Step-by-step implementation tasks. |
| Plan | How accepted work lands as reviewable PR-sized slices. | Re-litigating the proposal, spec, or ADR. |
| Active goal | What agents are executing now and where to look next. | The entire operating board or a reason to ignore fresher repo/GitHub state. |
| Support tiers | What users may believe about supported surfaces and the proof behind claims. | Aspirational roadmap claims. |
| Policy ledger | Machine-readable rules, exceptions, owners, reasons, and review dates. | Unchecked prose-only policy. |
| CI | What was validated by concrete commands. | Claims that were not actually proven. |
| Closeout | What landed, what proved it, what changed, and what remains. | New scope or hidden follow-up commitments. |

## Required artifact seams

Keep one kind of truth in one artifact. Do not turn a proposal, spec, ADR, or
plan into a mixed bag of roadmap, acceptance tests, architecture, CI policy,
release notes, and PR queue.

Every durable artifact should state:

- status;
- owner;
- creation date;
- linked proposal, spec, ADR, or plan when applicable;
- support-tier impact;
- policy impact;
- required evidence;
- non-goals;
- claim boundary;
- rollback or exit path.

## Directory map

| Path | Purpose |
| --- | --- |
| `docs/source-of-truth/` | Repo operating doctrine and source-of-truth conventions. |
| `docs/templates/` | Required shapes for proposals, specs, ADRs, plans, closeouts, and PR bodies. |
| `docs/proposals/` | Accepted, active, rejected, or superseded problem statements. |
| `docs/specs/` | Behavioral contracts and evidence requirements. |
| `docs/adr/` | Durable architecture decision records. |
| `plans/` | PR-sized execution maps for accepted work. |
| `docs/handoffs/` | Human- and agent-readable handoffs or lane closeouts. |
| `docs/status/` | Support-tier and claim-boundary status documents. |
| `policy/` | TOML ledgers for machine-checkable repo policy when introduced. |

## Proof and claims

No stable claim should exist without a proof command. No promotion should land
without a support-tier update. No broad claim should be made from narrow proof.

When proof is advisory or incomplete, keep the claim boundary narrow and say
what remains before promotion.

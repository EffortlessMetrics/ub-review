# Source-only review

A pull request can carry more than the code it claims to change. Build scripts,
workflows, installers, hooks, plugins, tests, and project-controlled tools can all
cause code to execute when someone checks the contribution.

The proposed source-only Review Pipe investigates an unfamiliar change *before*
authorizing any candidate-controlled execution. It shares ub-review's review semantics,
but it does not own a CI gate, dispatch proof, approve a merge, or write source.

## What the review can do

The intended path admits an exact revision and a verified, bounded source/context
packet. Approved static instruments and model programmes examine the change and its
relevant consumers, including suspicious workflow and dependency behavior, potential
credential theft, and paths that could compromise a development machine or CI runner.

It can produce material findings, unanswered proof questions, and a useful PR review. A
model hypothesis is not executable proof. A clean review within its admitted scope is
not a CI PASS, a whole-repository safety certificate, or a claim that the source was
executed successfully.

Retaining bounded repository context across PRs and using inexpensive static/model
investigation avoids allocating a fresh candidate-execution VM just for first-line
screening. Actual throughput, cost, false positives, and adversarial recall still
have to be measured.

## Execution and trust boundary

Candidate execution is forbidden, including builds, tests, candidate-defined hooks,
package lifecycle scripts, plugins, macros, project language servers, locally defined
Actions, dynamic libraries, Docker builds, and model-generated commands. The
source-only reviewer cannot dispatch executable ProofIntents; they remain questions for
a separately authorized proof environment.

Trusted analysis code still executes. Git ingestion, parsers, static workers, bounded
context materialization, model prompts, and credential separation must therefore be
qualified against hostile input and prompt injection. A no-compile label doesn't
establish containment.

## Status and ownership

The source-only contract and standalone adapter are **planned, not a qualified operated
service**.
[ub-review #1334](https://github.com/EffortlessMetrics/ub-review/issues/1334) owns
source admission, [#1338](https://github.com/EffortlessMetrics/ub-review/issues/1338)
owns canonical judgment reuse, and
[#1349](https://github.com/EffortlessMetrics/ub-review/issues/1349) owns the unresolved
activation-dependency decision.

The [em-ci tower qualification](https://github.com/EffortlessMetrics/em-ci/issues/189)
covers persistent hardware, hostile-source containment, throughput, and evaluation; its
[operated advisory service programme](https://github.com/EffortlessMetrics/em-ci/issues/213)
has distinct qualification and authorization. Neither programme creates a second review
engine or changes ub-review's CI authority.

For present nonblocking use, see [advisory evaluation](ADOPTION_ADVISORY.md) and
[Product state](PRODUCT_STATE.md).

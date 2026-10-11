# ub-review

Agentic software production can turn out changes faster than traditional CI and review can economically handle. Builds, tests, model calls, and maintainer attention still cost something. An unfamiliar PR creates another problem: running its build scripts or tests before inspecting them gives that code a chance to execute.

`ub-review` is being built for two jobs: **efficient, effective CI** and **first-line security review of untrusted contributions without executing their code**. Both paths should also produce an excellent PR review using much of the investigation they already need.

[Try the current advisory path](#inspect-before-adopting) · [Current capabilities](docs/PRODUCT_STATE.md)

## Intelligent CI

A PR needs the checks its repository requires. It doesn't need independent jobs and reviewers repeatedly discovering the same facts or running equivalent work.

We're designing the CI path to start Required work promptly, reuse approved executions and their evidence, and spend additional compute where static analysis or model investigation finds a question worth testing. Builds, focused tests, sanitizers, and other proof still have a place; they should be selected for what they establish.

The goal is better verification with less wasted runner time and model spend. We'll measure latency, total cost, review quality, and missed defects separately. Saving compute by silently skipping Required evidence wouldn't be much of an improvement.

## Review untrusted code before running it

A PR can change build scripts, workflows, dependencies, and tests. If our first move is to run them, we've already given the contribution more authority than it needed for an initial review.

The planned source-only path examines a bounded representation of the repository using approved static tools and model investigation. Candidate-controlled code does not run: no builds, tests, hooks, plugins, project language servers, or generated shell commands. It cannot dispatch executable proof or issue a CI pass.

Persistent repository context and inexpensive inference make it practical to investigate more changes without allocating a candidate-execution VM to every PR. That's useful for catching suspicious workflow changes, credential theft attempts, and other ways a contribution could compromise a developer or CI runner before someone runs it.

Hostile source is still hostile input to parsers, tools, and models. Containment and detection quality need their own evidence. [Source-only review design and status](docs/SOURCE_ONLY_REVIEW.md).

## An excellent PR review from either path

Both paths already need to understand what changed, what it affects, and where the evidence is weak. We want the maintainer to get one useful GitHub review from that work: material findings, specific verification questions, and evidence that changes the decision. They shouldn't have to read the status report from every model and sensor.

A small additional investment in review can be worth it when it catches something important. We measure that benefit alongside the extra cost rather than optimizing purely for the fewest model calls.

**A concrete review-output replay:** the [perl-lsp-swarm #3627 fixture](docs/REVIEW_EXPERIENCE_FIXTURES.md) starts with existing parser-review threads. Instead of duplicating the finding, its expected public output replies to the relevant thread:

> Confirmed by focused execution: the later subscript case fails before the patch and passes on this head.

On the fixed head, the expected public output is silent. This is a deterministic replay with recorded inputs and a fake GitHub transport, not a live model-review benchmark.

## How the work fits together

The shared design binds investigation to an exact code revision and reuses repository facts and change context. Static tools and model programmes can raise findings or questions. The CI path can run approved proof and connect its receipts to the claims they actually test; the source-only path records unresolved proof questions without executing them.

Review judgment, successful GitHub delivery, and deterministic CI sufficiency remain different results. Full tool output, model traces, costs, and receipts stay in artifacts. The maintainer gets the findings that matter.

[Architecture](docs/ARCHITECTURE.md) covers the execution and authority contracts. [CI efficiency design](docs/CI_EFFICIENCY_PATH.md) covers the intended scheduling and reuse model.

## Inspect before adopting

**Current availability:** revision-bound evidence collection, sensors, bounded model investigation, focused proof paths, and grouped-review compilation exist. The unified CI scheduler, externally calibrated sole required check, and operated source-only security service are still being built. [Product state](docs/PRODUCT_STATE.md) distinguishes implemented code, retained proof, and trusted authority.

For a first run without provider secrets or GitHub write permissions, use the [model-off, artifact-only advisory evaluation](docs/ADOPTION_ADVISORY.md) and keep existing required CI. That evaluates evidence collection; it does **not** demonstrate the model reviewer.

With the CLI installed in a clean GitHub checkout with an `origin` remote, you can instead generate an advisory workflow and inventory existing CI. `enable --inspect` **writes** `.github/workflows/ub-review.yml` and `.ub-review.toml`; it isn't a dry run. Start only when those paths do not already exist:

```bash
ub-review enable --inspect --mode advisory --model minimax
ub-review audit-ci
```

Review the generated files before committing them. The MiniMax option selects a model-on template, not a credential-safe source-only deployment. Do not expose provider or posting credentials to candidate-controlled execution. `audit-ci` writes local inventory receipts without altering existing CI or branch protection.

`init` is an alternative configuration path, **not** a step before `enable`; the generator refuses to overwrite an existing setup. [Getting started](docs/QUICKSTART.md) covers installation, manual configuration, and the separate CI migration path.

## Documentation

- [First-run advisory evaluation](docs/ADOPTION_ADVISORY.md) · [CLI and Action reference](docs/ACTION_REFERENCE.md)
- [Review output and delivery](docs/REVIEW_BODY_CONTRACT.md) · [Packet artifacts](docs/ARTIFACTS.md)
- [Source-only review](docs/SOURCE_ONLY_REVIEW.md) · [Architecture](docs/ARCHITECTURE.md)
- [Bun/native-boundary preset](docs/GH_RUNNER_BUN.md) · [Current product state](docs/PRODUCT_STATE.md)

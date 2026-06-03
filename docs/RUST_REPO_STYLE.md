# Rust repo style

`ub-review` is a Rust-first repository. That does not mean every byte in the
repository must be Rust. It means Rust is the default construction material for
production logic, repository automation, test harnesses, fixture runners,
release checks, policy checks, and reporting tools.

The house rule is:

```text
default rule
-> narrow exception
-> owner
-> reason
-> machine check
-> report
-> CI gate
-> reviewer-visible receipt
```

Non-Rust files are allowed only when they are the right adapter surface: GitHub
workflow declarations, GitHub Action metadata, docs, fixtures, pinned config,
assets, lockfiles, or consumer-facing examples. A new non-Rust surface should be
reviewed as an exception to the default, not as a casual convenience.

## Doctrine

```text
Rust is the default construction material. Repository policy belongs in Rust
and, once this repo grows an xtask crate, xtask is the control plane.

Non-Rust files, generated files, dependency surfaces, process spawning, network
access, workflow shell, executable bits, lint suppressions, local-context
examples, and panic-family calls are all exceptions.

Exceptions must be narrow, owned, reasoned, mechanically checked, and reviewed.
Broad path allowlists control where files may exist; companion behavior checks
control what those files may do. For Rust call-site policy, prefer AST-backed
semantic identity over line numbers. Clippy provides fast feedback;
repo-specific Rust checks are authoritative. Tests do not get blanket carveouts.
Automation may normalize and report; humans approve new exceptions.
```

## Rust-first, not Rust-only

Use Rust for:

- production CLI and action behavior;
- deterministic packet generation and validation;
- provider adapters and response parsing;
- policy checks and release checks;
- test harnesses and fixture runners where practical;
- reporting tools that decide repository state.

Use non-Rust only as an owned adapter surface. Valid examples include:

- `.github/workflows/*.yml` for GitHub workflow declarations;
- `action.yml` for GitHub Action metadata;
- Markdown documentation and prompt templates;
- JSON/TOML/YAML fixtures and configuration;
- lockfiles and generated goldens when the generation path is documented;
- tiny shell snippets that demonstrate consumer setup rather than govern repo
  policy.

Avoid adding shell, Python, JavaScript, or TypeScript as repository machinery
when the behavior belongs in Rust. If a non-Rust helper is necessary, the PR
must explain why Rust is not the right place, who owns the surface, what kind of
surface it is, and what command checks it.

## Exception receipts

Every meaningful exception needs a reviewer-visible receipt. The receipt should
answer all of these questions:

| Question | Required answer |
|---|---|
| What is allowed? | A narrow path, glob, selector, pattern, or count. |
| What kind of exception is it? | A classification such as fixture, workflow, generated, dependency, process, network, lint, or panic-family. |
| Who owns it? | A repo surface owner or maintainer role. |
| Why is it accepted? | A concrete reason explaining why the default path is not suitable. |
| What checks it? | A command, CI job, fixture test, or policy check. |

Do not add allowlist entries, lint suppressions, or policy exceptions with vague
reasons such as "needed for tests" or "temporary." A good reason explains the
risk boundary and the intended lifetime.

## Panic-family and lint policy

Panic-family calls (`unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, and
`unreachable!`) are not normal control flow. The workspace denies the main
panic-family Clippy lints, and tests do not receive a blanket carveout.

When a fail-fast path is justified, prefer a narrow, documented exception over a
broad suppression. A useful exception includes:

- the exact call site or semantic selector;
- the family of panic behavior;
- a classification such as `test_only`, `fixture_setup`, or `invariant_checked`;
- an explanation of why returning an error would make the code less clear or
  less useful;
- the check that prevents the exception from spreading.

General-purpose lints are fast feedback. Repository-specific Rust checks are the
authority for policy that needs repo context.

## Semantic identity over incidental location

Line numbers are useful locators, but they are poor policy identities. For Rust
call-site policy, prefer semantic selectors such as container, callee, macro
name, receiver fingerprint, or associated function over `path + line + column`.
Harmless movement should not churn receipts. Semantic drift should fail.

Text search can be a first pass. AST-backed evidence is preferred when identity
matters.

## Behavior policies for mixed surfaces

A file being allowed to exist does not mean behavior inside that file is
unrestricted. Treat the following as separate policy surfaces:

- generated files;
- dependency and lockfile changes;
- process spawning;
- network access;
- workflow shell blocks;
- executable bits;
- local machine or session context;
- lint suppressions;
- panic-family calls.

Each behavior surface should have its own receipt and its own check. As the repo
adds an `xtask` control plane, these checks should move into Rust rather than
accumulating more shell or Python.

## Automation boundaries

Automation may normalize, sort, format, detect, report, and fail. It must not
silently bless new risk. In particular:

- formatting or shape commands do not approve exceptions;
- generated proposals need human-written explanations;
- broad cleanup should be split from policy changes;
- CI receipts should make the accepted state easy to review.

## PR evidence packet

A good `ub-review` PR is an evidence package, not just a diff. It should make
these sections easy to find:

- production delta;
- evidence or support delta;
- acceptance criteria;
- review map;
- policy checks;
- commands run;
- non-goals.

When adding non-Rust, generated, dependency, process, network, executable, lint,
local-context, or panic-family surfaces, update the matching documentation or
receipt and run the matching check before requesting review.

## Current repo application

This repository already encodes part of the style through Rust 2024, workspace
lint policy, deterministic fixtures, and a CI ladder for format, check, tests,
Clippy, and docs. The next tightening steps should be narrow and reversible:

1. keep new repository machinery in Rust;
2. add an `xtask` crate before adding more policy scripts;
3. introduce explicit allowlist files only when there is a checker to enforce
   them;
4. migrate existing helper scripts into Rust when they begin deciding repo
   policy rather than verifying consumer artifacts;
5. make each policy PR carry the receipt schema, command, report, and CI gate
   together.

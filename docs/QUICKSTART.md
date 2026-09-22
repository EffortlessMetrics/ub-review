# Evaluate ub-review without changing merge authority

`ub-review` combines revision-bound tool evidence, bounded model investigation,
approved proof, and one grouped review. Models investigate; receipts decide CI.
The live authority and release boundaries are recorded in
[Product state](PRODUCT_STATE.md), [build readiness](development/BUILD_READINESS.md),
and [the release runbook](RELEASE_RUNBOOK.md). A command or preset existing does
not prove that its deployment posture is ready.

**Start with a non-required, model-off, artifact-only evaluation. Keep existing
required CI.** The [manual advisory guide](ADOPTION_ADVISORY.md) provides the
workflow/config pair without provider secrets or GitHub write permissions.
This is an engineering evaluation, not a supported sole-gate rollout or a
promise that every run completes within a fixed time or artifact budget.

## 1. Choose and inspect the exact tool identity

Use a reviewed full 40-character Action commit SHA. A merged SHA alone is not a
support certificate. Record the candidate SHA, exact checks and retained packet,
platform, installation method, and rollback pin that justify the selection.

Published archives and development source are different things. The
[release runbook](RELEASE_RUNBOOK.md#current-state-and-ownership) records the
historical `v0.1.0` and `v0.1.1` publications; refresh live metadata before
choosing a release. Package version `0.1.2` by itself proves neither a published
tag nor an installable artifact. Do not rerun or overwrite a historical release
to obtain a new candidate.

For an explicit development-source installation, in a clean checkout of the
selected `ub-review` commit:

```bash
git rev-parse HEAD
cargo build --release --locked
./target/release/ub-review --version
./target/release/ub-review --help
```

Use that checkout's pinned Rust toolchain. A successful source build does not
prove release-only installation, another operating system, or an older glibc
baseline. The release-only path must independently prove exact asset identity,
checksum, runtime compatibility, and absence of source fallback.

## 2. Use the contained first-run recipe

Follow [ADOPTION_ADVISORY.md](ADOPTION_ADVISORY.md) in a disposable repository or
an isolated evaluation branch. Review both files before committing them.
The first run has no provider keys, no posting, no OIDC, and no PR/check write
permission. It does not alter branch protection or replace existing workflows.

Inspect the packet even when the workflow is green. At minimum, record the
reviewed revision, applicable obligations, actual receipts, missing evidence,
legacy gate conclusion, and posting disposition. TaskLedger and publication
shadow reports are diagnostics at the current frontier, not an authoritative
`FinalizedOutcome`. Never relabel missing evidence as a clean review.

## 3. Understand what `enable` currently generates

The generator can help inspect a repository, but its output is a proposal to
review, not a security-approved deployment. Run it only in a clean disposable
checkout or branch where newly created workflows will not execute until review:

```bash
ub-review enable --mode advisory --model minimax --inspect
```

Current `enable` accepts MiniMax for its `--model` option; it is not the
model-off generator. Use the manual model-off recipe for the first run. The
advisory preset does not itself remove provider inputs, write permissions, or
posting from the generated workflow.

The current resolver in [src/enable.rs](../src/enable.rs) prefers a release when
lookup succeeds. It checks the release tag and expected asset names. That is
not a deployment-readiness verdict or proof of the archive's runtime behavior.
The generated release path uses the release tag in `uses:`. Resolve and review
its full Action SHA separately; a tag is not an immutable pin.

When release lookup is unavailable, an explicit source fallback is possible:

```bash
ub-review enable --mode advisory --model minimax --action-sha <40-hex-sha>
```

`--action-sha` is a fallback: it does not override a successfully resolved
release. Inspect the emitted `uses:`, `install-mode`, `release-version`,
permissions, provider inputs, and config. Do not use `--force` to overwrite an
existing setup without reviewing the exact delta and preserving rollback.

## 4. Add model review only after reviewing its trust boundary

Model review should remain advisory. Grant credentials only to trusted
reviewer/publisher execution, not to an untrusted checkout or its build scripts,
tests, tools, or candidate-controlled configuration. Putting a secret in
`with:` instead of `env:` does not create process isolation.

A same-repository PR can still contain unsafe code. A fork guard and
`persist-credentials: false` reduce particular exposures but do not establish
that all code executed in a credential-bearing job is trusted. Fork secrets and
token permissions also depend on event and repository settings. Do not switch
to `pull_request_target` merely to obtain secrets for candidate execution.

The intended released-coordinator and publisher separation remains tracked by
[#658](https://github.com/EffortlessMetrics/ub-review/issues/658). Until that
boundary is proven for the chosen deployment, retain model-off containment or
an explicitly reviewed trusted-only pilot, not a generic credential-bearing
PR workflow. See the [GitHub security guidance](#security-references).

## 5. Diagnose and retain evidence before promotion

| Symptom | Evidence to inspect | Safe response |
| --- | --- | --- |
| No review appears | Skip receipt, prepared payload, post result/error and delivery transaction, all bound to the current head | Expected quiet, artifact-only, and failed delivery are different states. Do not infer posting from preparation. |
| Tool or proof unavailable | Sensor status, proof receipt, argv, exit/timeout, platform/tool version | Report missing evidence; retain existing CI. Do not create a code defect or PASS from absence. |
| Analyzer flags apparently tested behavior | Exact analyzer version, canonical diff and digest, finding ID, source path, reproducer and discriminating assertion | Investigate upstream and keep genuine gaps separate. Do not blanket-suppress findings to qualify a release. |
| Large or stalled artifact upload | Compressed and expanded size, dominant member, output limits and task deadline | Stop expansion/rollout and retain bounded diagnostics. A timeout is not a byte limit. |
| Archive cannot execute | Binary version, OS/architecture/libc, asset ID and digest, install receipt | Restore the verified prior pin. Do not silently compile from source under release-only policy. |

For a support report include the exact Action/tool SHA, reviewed candidate and
merge identities, run ID and attempt, artifact ID/digest, sanitized effective
config, expected/observed behavior, and smallest reproducer. Do not publish
provider keys, raw private prompts, private source, or whole packets containing
sensitive data. Preserve raw evidence privately; label redacted copies and
retain their own digests rather than presenting them as original bytes.

[Adoption modes](ADOPTION_MODES.md#staged-promotion-checklist) distinguishes
available knobs from earned authority. A low-noise window or a recommendation
command is not permission to retire CI, make the tool the sole required check,
enable model-derived blocking, or publish a new release.

## Security references

Reviewed against GitHub's primary guidance on 2026-09-16:

- [Secure use reference](https://docs.github.com/en/actions/reference/security/secure-use): least privilege, full-SHA pins, untrusted code, and artifact handoffs.
- [Securely using pull_request_target](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target): privileged execution is a separate trust decision.

## Related

[Manual advisory evaluation](ADOPTION_ADVISORY.md) ·
[Mode/reference boundaries](ADOPTION_MODES.md) ·
[Runtime profiles](RUNTIME_PROFILES.md) ·
[Release preparation](RELEASE_RUNBOOK.md) ·
[Fresh-repository acceptance #811](https://github.com/EffortlessMetrics/ub-review/issues/811).

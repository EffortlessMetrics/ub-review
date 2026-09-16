# Adoption modes: available controls and earned authority

**Advisory-first; retain existing required CI.** A preset existing in the CLI or
Action does not make that posture deployment-ready. [Product state](PRODUCT_STATE.md)
owns earned capability and [issue #945](https://github.com/EffortlessMetrics/ub-review/issues/945)
owns execution order. This reference does not grant required-check, release,
review-forward, or sole-gate authorization.

For the initial model-off, artifact-only workflow with no provider or write
credentials, use [ADOPTION_ADVISORY.md](ADOPTION_ADVISORY.md). For generator,
release identity, and support boundaries, use [QUICKSTART.md](QUICKSTART.md).

## Quick start: pick a `review-mode`

The three convenience presets map to the legacy controls below. This is a
configuration reference, not a support-tier table:

| `review-mode` | `mode` | `fail-on-gate` | `review_forward` | Current adoption posture |
| --- | --- | --- | --- | --- |
| `advisory` | `review-byok` | `false` | `false` | Starting posture; keep non-required and review credentials/posting separately. |
| `gate` | `intelligent-ci` | `true` | `false` | Deterministic-floor implementation path; not a recommendation to replace current required CI. |
| `strict` | `intelligent-ci` | `true` | `true` | Legacy opt-in model-derived blocking; not the default or a supported first rollout. |

When supplied, the preset overrides the legacy `mode`, `fail-on-gate`, and
`[gate].review_forward` controls, with warnings for overridden settings. Leaving
`review-mode` unset uses those controls directly. These options do not change
GitHub branch protection, establish credential isolation, or independently
verify current-head receipts.

## The four modes

Advisory is non-blocking review/evidence collection. Deterministic-floor uses
proof, sensors, and configured policy. Hybrid is the same deterministic posture
with explicit tool-gate thresholds. Review-forward additionally allows the
final reporter verdict to affect legacy enforcement under explicit opt-in.
These are behavior distinctions, not four levels of proven product readiness.

Individual investigation lanes do not directly post, execute arbitrary commands,
or establish deterministic CI truth. The intended final-lead judgment and
receipt-based outcome must remain separate: model suspicion is not deterministic
failure; model silence is not a satisfying Required receipt.

### Advisory

Keep the job non-required. Model-off and artifact-only are valid evaluation
choices and require no provider key. Model-on posting is a separate trusted
pilot, not an automatic consequence of selecting `advisory`.

Do not use `continue-on-error` as proof of healthy execution. It can mask the
check conclusion while installation, evidence, or publication failed. Read the
actual receipts and distinguish expected quiet, disabled model review, missing
evidence, failed execution, and failed delivery.

### Deterministic-floor and hybrid

`fail-on-gate: true` enables the current enforcement path. It does not prove
that every Required obligation was correctly selected, executed, reconciled,
or satisfied. The current legacy `gate_outcome.conclusion` must not be described
as the already-authoritative future `FinalizedOutcome`.

The intended CI contract is `PASS`, `FAIL`, or `NOT_PROVEN`: deterministic
violations, evidence unavailability, review judgment, and delivery failure are
separate facts. The #957–#962 authority packet, result-plane non-interference,
and later explicit enforcement migration must be proven before promoting those
claims. Retain legacy diagnostic fields and their disagreements during shadow
comparison rather than rewriting them to appear consistent.

`[[proof.required]]` and `[tools.<id>.gate]` remain configuration surfaces for
repository-owned obligations and thresholds. No command spelling, successful
summary, or tool exit alone establishes equivalence to existing CI. Analyzer
false positives require reproducer-driven disposition, not blanket suppression
or automatic threshold relaxation.

### Review-forward

The legacy `[gate].review_forward = true` path may add a blocking reason when
the reporter verdict is `changes_requested` or `uncertain`. This is
probabilistic review policy, not deterministic proof. Leave it disabled for
initial deployments. Final-lead authority, calibrated finding classes, truthful
delivery, and explicit owner authorization are separate prerequisites; a
positive calibration recommendation does not complete them.

### Posting posture (`summary_only_body`)

Posting is independent of deterministic evidence sufficiency:

| Setting | Configured public-output intent |
| --- | --- |
| `suppress` | Keep model findings in artifacts; do not rely on posted-review metrics. |
| `post_substantive` | Prepare a grouped review for substantive findings; suppress lane-status boilerplate. |
| `post_all` | Broader classified output; requires deliberate calibration and review. |

The chosen tool version, `posting` input, terminal-state policy, and actual
GitHub delivery receipts still govern the observed result. None of these values
proves a payload was delivered. With artifact-only or suppression, zero acted-on
comments is expected and is not a measurement of reviewer usefulness.

## Staged promotion checklist

These are evidence requirements, not automatic upgrades after a number of PRs.
They interpret the canonical roadmap for adopters without changing its order.

| Stage | Evidence required before expansion | Authority retained |
| --- | --- | --- |
| Contained evaluation | Exact identity; installed tool behavior; model-off packet; explicit gaps; sizes/timeouts; failure diagnostics. | Existing required CI. No provider/write credentials in candidate execution. |
| Model-on advisory pilot | Reviewed credential boundary; valid provider output; proof-to-claim links; actual delivery; human dispositions and material misses; cost/latency. | Existing required CI. Model review stays non-required. |
| Deterministic co-required beta | Coherent #957–#962 packet; bounded output; Required-first plan/scheduler; #1277 non-interference; #1275 comparison; explicit #1015 enforcement proof. | Existing independent CI alongside the candidate gate; no inferred job retirement. |
| Stable sole gate | Released stable coordinator; hostile-head isolation; terminal watchdog; independent receipt checks; exact-byte distribution; external calibration; rollback/break-glass; explicit owner approval. | Repository owner selects branch-protection changes only after the acceptance in #658. |

A model-on advisory pilot is not required to prove a complete model-off gate.
Conversely, a model-off gate does not prove reviewer value. Keep these acceptance
planes separate even when one packet supplies evidence for both.

Issue [#811](https://github.com/EffortlessMetrics/ub-review/issues/811) owns the
fresh-repository installation/generation/upgrade/rollback proof. The
[release runbook](RELEASE_RUNBOOK.md) owns candidate preparation and separate
publication authorization. This checklist closes neither programme.

## Calibration → promotion commands

The existing helpers remain useful diagnostics:

```text
ub-review status --run-dir <run>
ub-review recommend --runs-dir <dir>
ub-review promote --runs-dir <dir>
```

Their calibration-based suggestions are not branch-protection or release
permissions. A sample-size threshold, low reported noise, or one acted-on
comment cannot substitute for current-revision receipt integrity, trusted
execution, complete terminalization, portability, or rollback proof.

Preserve the original packet and digest before adding human labels. Record
labelled/redacted derivatives with their own identity, and retain dispositions
separately where practical. Never edit a source receipt to make an old run
satisfy a new revision. Report unmeasured fields as unmeasured.

Track accepted, invalid, duplicate, deferred, and missed findings; proof-changed
conclusions; expected-quiet behavior; delivery success/failure; provider and
runner failures; process/queue/wall time; provider cost; and packet size. Useful
public actions and reproducible evidence matter more than comment volume.

## Related

[Manual evaluation](ADOPTION_ADVISORY.md) · [Quickstart](QUICKSTART.md) ·
[Runtime profiles](RUNTIME_PROFILES.md) · [Tool policy](POLICY_ALLOWLISTS.md) ·
[GitHub secure use](https://docs.github.com/en/actions/reference/security/secure-use).

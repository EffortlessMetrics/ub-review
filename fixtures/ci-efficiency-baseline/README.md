# CI-efficiency baseline corpus

_Status: draft scaffold for issue #1270. This file is not a measurement receipt._

This directory will retain bounded, source-linked before-state observations for
the model-off CI-efficiency programme. It must preserve distinct meanings for:

- exact repository, revision, workflow, run, attempt, job, runner, and artifact
  identity;
- elapsed wall time, critical-path time, summed process time, billable estimate,
  and Linux-equivalent runner time;
- compressed artifact bytes and expanded logical packet bytes;
- known values, explicit unknowns, and measurements that could not be obtained;
- confirmed execution identity versus `unproven_equivalence` candidates;
- current additive outcome fields versus legacy enforced compatibility fields.

The completed corpus should reference the retained authority-incident manifest
for PRs #915, #916, and #921 rather than duplicating it. It should also retain
exact source metadata for the final #1263 and #1266 runs and other representative
clean, deterministic-failure, unavailable-evidence, changed-test, and
multi-package shapes where evidence exists.

Raw workflow archives, unbounded logs, provider prompts, credentials, and
private data do not belong in this directory. Every measurement must be
recomputable from a named immutable source or remain explicitly unknown.

The implementing PR must add a versioned machine-readable manifest, a bounded
human inventory, independent validation, negative fixtures, privacy and size
checks, and the exact source/claim boundary required by issue #1270. Until those
artifacts land and verify, this directory proves only that the remote draft lane
exists.

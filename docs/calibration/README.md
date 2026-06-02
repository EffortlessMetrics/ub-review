# Calibration

Calibration records how real `ub-review` runs changed review behavior.

This directory is not a changelog and not a release checklist. It tracks review
quality: acted-on findings, false premises, parked follow-ups, duplicated
comments, and prompt or compiler follow-up.

## Entry shape

Use this structure for each run:

```md
## PR #N short name

Date: YYYY-MM-DD
Repo: owner/repo
Artifact: name or digest
Runtime: XmYs
Inline comments: N
Model lanes: N ok, N failed

Acted on:
- ...

Dismissed:
- ...

Parked follow-ups:
- ...

Prompt/compiler follow-up:
- ...
```

## Manual tags

When humans reply to findings, prefer stable tags that can be scraped later:

```text
ub-review: acted-on
ub-review: dismissed-false-premise
ub-review: parked-follow-up
ub-review: duplicate
```

Do not treat a calibration note as proof that a lane is good or bad. It is a
receipt for later tuning.

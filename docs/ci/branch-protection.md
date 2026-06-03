# Branch protection

The preferred required status is the aggregate `ci/merge-gate` job.

Avoid requiring optional leaf jobs directly, such as platform canaries, coverage, mutation testing, Docker, model review, or advisory static analysis. Optional jobs may be selected by labels or risk packs, but skipped optional checks should not deadlock unrelated PRs.

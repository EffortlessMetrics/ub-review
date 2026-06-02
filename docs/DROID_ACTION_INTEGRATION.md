# Droid Action integration

`ub-review` can post one grouped Pull Request Review from validated findings.
Droid can still consume the packet as a downstream review actor, but sensors and
lanes should not post directly.

Recommended Droid behavior:

- full six-lane matrix on draft PR opened
- full six-lane matrix on `ready_for_review`
- no `synchronize` spam
- docs-only PRs skipped
- `mention_trigger_user: false`
- lane and model displayed separately
- no issue-comment spam or one-comment-per-lane posting

Completion display:

```text
Lane: security
Model: MiniMax-M3
```

Inline prefix:

```text
[security]
```

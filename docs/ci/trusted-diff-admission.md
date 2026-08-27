# Trusted-base explicit diff admission

`ub-review` can construct its review context from a clean, base-owned checkout
without resolving or checking out the pull-request head. This is the first
trusted-base admission seam tracked by issue #882 and the hostile-head roadmap
in #876.

Supply all four inputs together:

- `--trusted-base-tree`: exact Git tree SHA of the base-owned root;
- `--trusted-head-sha`: exact candidate commit SHA used only as an identity
  label;
- `--trusted-changed-files`: newline-delimited paths, strictly sorted and
  duplicate-free;
- `--trusted-diff-patch`: a patch applicable to the supplied base tree.

The composite action exposes the same names as inputs and automatically selects
dry-run packet construction when `trusted-base-tree` is present. Direct CLI
use requires `--dry-run`, `--model-mode off`, `--posting artifact-only`, and no
`--allow-heavy`; sensor execution against the base checkout, secret-backed
model execution, credentialed GitHub delivery, PR-thread/prior-receipt
collection, and candidate proof execution are intentionally outside this
child seam.

The changed-path object is capped at 1 MiB and the patch at 64 MiB. Both must
be valid UTF-8. Admission copies the exact validated patch bytes into its
private scratch directory before invoking Git, so a caller cannot swap the
external file between identity hashing and tree derivation.

Admission fails closed unless the worktree is clean and its `HEAD` tree equals
the supplied base tree. Paths must be normalized repository-relative slash
paths. The patch is applied only to a temporary Git index and temporary object
directory seeded from the verified base tree. The resulting candidate tree and
changed-path set must agree with the supplied changed-path object. The
temporary index and objects are removed after admission, and the candidate head
object is never resolved or loaded.

The ordinary `input/revision-admission.json` remains the identity receipt. Its
canonical identity binds the base commit/tree, supplied head SHA, derived head
tree, changed-path digest, and exact patch digest. `input/diff-context.json`,
`input/changed-files.txt`, and `input/diff.patch` are then written from the
validated explicit objects.

Example trusted invocation:

```text
ub-review run \
  --root /trusted/base-checkout \
  --trusted-base-tree "$BASE_TREE" \
  --trusted-head-sha "$HEAD_SHA" \
  --trusted-changed-files /trusted/input/changed-files.txt \
  --trusted-diff-patch /trusted/input/diff.patch \
  --model-mode off \
  --posting artifact-only \
  --dry-run
```

The trusted workflow should generate changed paths with rename detection
disabled and sort them bytewise, matching the admission comparison:

```text
git -c core.quotePath=false diff --name-only --no-renames "$BASE_SHA" "$HEAD_SHA" | LC_ALL=C sort -u
git diff --binary --no-renames "$BASE_SHA" "$HEAD_SHA"
```

This contract proves trusted-base diff admission only. It does not yet provide
full hostile-head safety, environment isolation, immutable tool bootstrap,
job separation, safe credentialed model execution, or trusted GitHub posting.
Those boundaries remain open under #876.

# Repository bootstrap

Target repository:

```text
https://github.com/EffortlessMetrics/ub-review
```

The public GitHub page currently shows this repository as empty. This scaffold is intended to become its first commit.

## Bootstrap commands

```bash
git clone git@github.com:EffortlessMetrics/ub-review.git
cd ub-review
cp -R /path/to/ub-review-repo-ready/. .
cargo generate-lockfile
git add .
git commit -m "initial ub-review action scaffold"
git push origin main
```

## After bootstrap

In the Bun fork, add:

```text
.github/workflows/ub-review-packet.yml
```

using `examples/bun/.github/workflows/ub-review-packet.yml` from this repository.

The Bun fork should call:

```yaml
uses: EffortlessMetrics/ub-review@main
```

Pin to a tag or SHA after the first green run.

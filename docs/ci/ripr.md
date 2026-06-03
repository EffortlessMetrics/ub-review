# ripr policy

`ripr` gives mutation-testing-lite value at static-analysis prices.

It does not run mutants or report killed/survived outcomes. It statically asks
whether the behavior changed in this diff appears exposed to a meaningful test
discriminator.

`ripr` is an advisory PR-time signal. It does not replace unit tests, property
tests, coverage, or runtime mutation testing. It shifts mutation-shaped feedback
left so ordinary PRs can see oracle-gap risk without paying the cost of a full
mutation run.

Suppressions must be represented in `policy/ripr-suppressions.toml` with owner,
reason, expiry, and the artifact or test plan that covers the risk elsewhere.

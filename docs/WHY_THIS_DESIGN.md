# Why this design

`ub-review` is not meant to beat generic review bots by commenting more, and it
is not meant to imitate fixed-job CI.

It is meant to be better for UB/native-boundary work because it changes the
shape of the work: the runner decides what the PR needs proven, runs the
relevant evidence streams, and posts only the review decision that follows from
the receipts.

```text
PR diff
  -> targeted evidence plan
  -> deterministic shared packet
  -> relevant sensors once
  -> lane-specific packets
  -> bounded MiniMax/OpenCode Go direct provider lanes
  -> focused proof receipts
  -> validated inline comments
  -> one serious Pull Request Review
  -> running summary
  -> full artifact packet
```

The scarce resource is CI, not tokens. The runner should build shared evidence
once, overlap model investigation with local proof, validate comments against
the diff, and post one grouped review rather than many lane comments.

## Design bets

1. Shared evidence beats independent rediscovery.
2. Cheap static receipts beat ungrounded prose.
3. Lane-specific packets beat one giant prompt.
4. Missing evidence must be explicit.
5. No finding is not approval.
6. Heavy witnesses require explicit policy.
7. Posting is one grouped PR Review compiled from grounded findings.
8. The runner does the work traditional CI would do, but chooses it like a
   reviewer.

## First wedge

The first production preset is `bun-ub` because Bun UB PRs have a clear review
shape:

- Rust/native boundary risk;
- resizable ArrayBuffer resize/detach/transfer/GC hazards;
- stale pointer/length and worker handoff risks;
- active view region vs whole backing store mistakes;
- tests that can reach code but fail to prove the changed behavior.

Generalization should come from additional presets, not from weakening the Bun
preset.

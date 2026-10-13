# Compiler-free local context cost (#1413)

`read_bounded_text_with_status` used the capture ceiling to allocate a zeroed
buffer before reading. A 4,856-byte ledger with a 64 MiB ceiling therefore
reserved 64 MiB of address space. Under a 64 MiB process address-space limit,
the actual `run` aborted, although the same input with the default 64 KiB
ceiling succeeded. `usize::MAX` caused a capacity-overflow panic even for an
empty regular file. The fix grows regular-file capture with bytes read, with
a bounded one-byte lookahead for the existing truncation marker and flag.
Nonregular inputs and unknown file types keep the prior single read, including
its ceiling-sized reservation, so an open stream can return available seed
bytes without waiting for EOF.

This changes local file capture only. GitHub API hydration/error handling
(#1410), provider completion admission (#1411), proof disposition (#1412),
proof-link ordering (#1378), and model-off summary semantics (#1373) retain
their owners. No subprocess, network, policy, toolchain, or gate change is
included.

## Inputs and method

The source baseline is `2fae0dc21c2e178cb8175101dacd666b7b8ccdeb`, tree
`2b18b72daa5a1f9c8c2ae98a227c83aaa964b88d`. A separate detached checkout built
the baseline with locked, offline Rust 1.95.0 and one Cargo job. The candidate
uses a separate target directory. Benchmark subject repositories are never
compiled: their PATH exposes only Git and df, and every run uses
`--dry-run --model-mode off --no-github-summary`.

Two ordinary source snapshots are measured:

- ub-review at that exact source baseline, with its actual #1402 docs diff
  (`777d86a0fef6702f76502dd1c959c97d43600682...2fae0dc`).
- The installed serde_json 1.0.150 package source, carrying upstream VCS
  identity `a1ae73ac6a6940a4a57c673aebaa13ed4dfe3e8c`. Its locally frozen tree
  is `e95692a43756432f58272c6bbbe515c2de8b7ba1`.
  This control has an empty diff; it is a package snapshot, not a complete
  upstream Git-history checkout.

Setup runs `init` once per source snapshot, writing configuration and guide
outside the subject. Context hydration replays the connected #1402 title/body
snapshot through `GITHUB_EVENT_PATH`; the serde_json control deliberately uses
the same seed, not a claim about a serde_json PR. No live HTTP or provider call
occurs. Each before/after cell has three serial fresh-process runs. The same
4,856-byte ledger and Git roots are retained for both binaries.

`wait4` records wall time, child CPU, and largest-process RSS. A 1 ms Linux
`/proc/<pid>/status` sample records the parent process's observed `VmPeak`;
this sampling can miss a transient peak. Packet storage is expanded logical
file bytes, not compressed transfer cost. A separate consumer control applies
`RLIMIT_AS=64 MiB`; it is a campaign limit, not a product-installed budget.

## Results

Medians for hydrated runs with a 64 MiB local ledger ceiling:

| Subject | Before / after wall seconds | Before / after CPU seconds | Before / after VmPeak KiB | Before / after RSS KiB | Before / after packet bytes |
|---|---:|---:|---:|---:|---:|
| ub-review docs diff | 0.0946 / 0.0901 | 0.0916 / 0.0866 | 86,392 / 21,236 | 16,788 / 16,704 | 599,588 / 599,588 |
| serde_json empty diff | 0.0771 / 0.0757 | 0.0740 / 0.0721 | 86,392 / 21,232 | 16,764 / 16,772 | 400,277 / 400,277 |

Setup takes 0.0202/0.0170 seconds and writes 17,290/16,817 bytes of config and
guide respectively. Hydrated packets contain 203/183 files. Default-ceiling
hydrated medians before the fix are 0.0911/0.0763 seconds, 16,640/16,704 KiB
RSS, and 599,557/400,247 packet bytes. Unhydrated runs and individual repeats
are retained in the raw receipts.

The measured benefit is about 63.6 MiB less virtual address space and successful
admission for a small file under a constrained process limit. RSS, runtime,
and packet storage show no meaningful improvement. Lazy zero-page commitment
explains why the old allocation did not add 64 MiB to physical RSS. Slight
packet byte differences come from run-specific metrics, paths, and timestamps;
they are not a storage-saving claim.

Under the same 64 MiB process limit, both subjects have these outcomes:

| Local ledger ceiling | Baseline | Candidate |
|---|---|---|
| 64 KiB | exit 0 | exit 0 |
| 64 MiB | SIGABRT: allocation of 67,108,865 bytes failed | exit 0 |
| usize::MAX (64-bit) | exit 101: capacity overflow | exit 0 |

The shared context, shared-context hash, PR-context JSON, diff patch, changed
files, and diff-context JSON match byte-for-byte across before/after for both
subjects at both finite ceilings: 24 independent artifact comparisons.
Literal Rust consumer oracles cover empty/tiny files with maximum ceilings,
zero/exact/over budgets, split UTF-8, invalid UTF-8, and unavailable files.
The two maximum-ceiling regressions fail on the tests-only predecessor
`c0d14fa3aec39f5cb64004e4a53d51be55163863`; unchanged controls pass.

The pre-merge discussion refresh found a stream regression in the initial
`043426145812c0813ef90e3ce2866ee8c868067d` candidate: a held-open FIFO delivered
23 bytes but `read_to_end` stalled instead of returning the current seed. The
exact-base CLI finishes in 0.225 seconds while its writer remains open;
the initial candidate needs a kill at the independent two-second deadline.
Merge was held and prior broad caller clearance was superseded. The revised
reader uses metadata from the opened file to select incremental capture only
for regular files. Four independent author-prototype controls now finish while
the writer remains open (0.175-0.210 seconds): budget 64, zero, exact 23, and
truncating 22, with literal excerpt/flag equality. A pure Rust reader that
errors on its second read adds a one-read invariant without process spawning.
These immediate prototype checks supplement the later exact-head independent
review; they are not substitutes for it. GitHub API hydration is unchanged.

## Identity and limits

Retained baseline executable SHA256:
`af35a0d0873e5a3c36f3ba7c33b4f7ffc5f85d3a79d69dae85862b56fc623285`.
Retained candidate executable SHA256:
`c615f334e6a8d677c70f94099a31d8c60eb6b791736302c1bdb8b3bca3bf97e1`.
Raw commands, snapshots, per-run packets, source checkouts, compilation logs,
and measurements are retained under `/workspace/static-cost-evidence` in the
execution environment. These local paths are not hosted artifact downloads.
An initial after run accidentally reused the base binary from a shared Cargo
target; its receipts are explicitly superseded, and no result above uses it.

This is current compiler-free packet construction, not standalone semantic
review service activation, proof execution, model acceptance, CI qualification,
live GitHub hydration acceptance, or publication. Default sensors are planned
but not executed, model lanes remain unavailable/off, and their evidence
cannot establish a clean review. No fifty-lane capacity, sustained-load,
cross-platform, or total-storage bound is inferred. Content allowed by a large
ceiling can still consume correspondingly large memory; the regular-file fix
avoids paying for bytes that were never read. Nonregular/unknown file types
retain their existing allocation/read behavior, including large-ceiling
memory costs. Truncation markers and lossy UTF-8 conversion
retain their existing output semantics and can exceed the raw-byte ceiling.

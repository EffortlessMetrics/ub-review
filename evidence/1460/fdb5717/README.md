# #1460 follow-up evidence

Candidate: fdb5717688eb266b7b7b427b64104bea6a8ff722
Tree: 4ab0d4eee0ac1f9d37181e663a5dfddb7682cf70
Parent: 46bfbd5e2a1f8d2a78d2aaecfaffcf7df78da564

The follow-up adds one missing overlap-removal control and clarifies the historical and current scope links. The existing exact-nine validator was correct; weakening it to <=9 had survived the prior tests. Independent fresh-compiled RED/GREEN now kills that weakening at the intended semantic control, and restoration passes all 11 tests. Other validator, fixture, source-projection and policy bytes remain unchanged. The final corpus is 64,625 bytes.

Native formatting, check, workspace tests (1,270 top-level passes plus one nested pass and one intentional ignore), strict Clippy and staged precommit passed. After the documentation clarification, all 11 focused tests reran. Docs, policy and artifact-verifier evidence remains from identical relevant source at 46bfbd5e. Final precommit with installed RIPR on PATH passes its advisory RIPR step. Cargo-allow, unsafe-review and ast-grep remain missing; actionlint was skipped.

Whole-PR RIPR 0.10.0 on the final clean head still reports 203 unsuppressed gaps (1,569 findings, 738 unknowns), with advisory exit 0. Diff SHA256 d690685edc7001bc2bfd204b81790d172197ad9331bbd3c6f577613156f95d9e matches the generated base-to-worktree bytes. This is not strict-zero clearance.

Frozen original-head triage had 81 declaration-subject, 65 same-target fallible-oracle, 56 foreign-target-contaminated and one confirmed missing-control buckets. These are not blanket false-positive dispositions. Four representative behavior mutants produced one survivor and three caught cases; 199 gap IDs lacked exact-position replay. No thresholds or suppressions changed.

The original run and evidence remain historical; this folder is the supplemental exact-head record. Keep the PR Draft pending finding disposition and hosted qualification. No savings, receipt reuse, corpus completion or release claim is made.

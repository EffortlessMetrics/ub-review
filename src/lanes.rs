//! Lane registry and routing: the width rosters, repo-declared lane merge,
//! and selector filtering (cleanup train step 12, pure code motion). Lane
//! doctrine lives in SPEC-0011; builtin default lanes live in builtin.rs.

use anyhow::Result;

use crate::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LanePlan {
    pub(crate) id: String,
    pub(crate) role: String,
    pub(crate) model: String,
    pub(crate) model_display: String,
    pub(crate) receives: Vec<String>,
    pub(crate) focus: String,
}

/// Merge repo lanes into a selected execution lane set: a repo lane replaces
/// a same-id builtin, otherwise appends. Applied after lane-width selection
/// so repo lanes execute at every width and diff class - the first wiring
/// pass only reached the plan artifact (PR #344's run proved the gap: plan
/// notes recorded the lanes, the executed set did not contain them).
pub(crate) fn merge_repo_lanes_into(lanes: &mut Vec<LanePlan>, repo_lanes: &[LanePlan]) {
    for repo_lane in repo_lanes {
        if let Some(existing) = lanes.iter_mut().find(|lane| lane.id == repo_lane.id) {
            *existing = repo_lane.clone();
        } else {
            lanes.push(repo_lane.clone());
        }
    }
}

pub(crate) fn review_lanes_for_args(plan: &Plan, args: &RunArgs) -> Vec<LanePlan> {
    let mut lanes = match (args.lane_width, args.provider_policy) {
        (20, ModelProviderPolicy::OpencodeGoWide) if plan.diff_class == DiffClass::SourceUb => {
            opencode_go_wide_lanes()
        }
        (width, _) => review_lanes_for_width(width, plan),
    };
    merge_repo_lanes_into(&mut lanes, &plan.repo_lanes);
    lanes
}

pub(crate) fn selected_review_lanes_for_args(plan: &Plan, args: &RunArgs) -> Result<Vec<LanePlan>> {
    let include = parse_selector_set(&args.selectors.lanes, "--lanes")?;
    let exclude = parse_selector_set(&args.selectors.except_lanes, "--except-lanes")?;
    filter_lane_plans(review_lanes_for_args(plan, args), &include, &exclude)
}

pub(crate) fn review_lanes_for_width(width: usize, plan: &Plan) -> Vec<LanePlan> {
    match plan.diff_class {
        DiffClass::SourceUb => match width {
            6 => plan.lanes.clone(),
            10 => standard_minimax_lanes(),
            20 => deep_minimax_lanes(),
            _ => plan.lanes.clone(),
        },
        DiffClass::SourceGeneral => source_general_lanes(),
        DiffClass::TestsOnly => tests_only_lanes(),
        DiffClass::WorkflowTooling => workflow_tooling_lanes(),
        DiffClass::DocsOnly | DiffClass::ArtifactOnlySmoke => Vec::new(),
    }
}

pub(crate) fn standard_minimax_lanes() -> Vec<LanePlan> {
    vec![
        model_lane(
            "ub-memory-lifetime",
            "Memory lifetime and pointer ownership review",
            &["tokmd", "unsafe-review", "ast-grep"],
            "Review lifetime, pointer, aliasing, ownership, and safety-contract risks at changed native seams.",
        ),
        model_lane(
            "ub-active-view",
            "Resizable buffer and active-view review",
            &["tokmd", "unsafe-review", "ast-grep"],
            "Check active view region vs whole backing store, ArrayBuffer/TypedArray resize/detach/transfer, and stale length snapshots.",
        ),
        model_lane(
            "ub-worker-handoff",
            "Worker handoff and async capture review",
            &["tokmd", "unsafe-review"],
            "Check JS-backed memory crossing worker, async, GC, detach, and transfer boundaries.",
        ),
        model_lane(
            "tests-red-green",
            "Red/green changed-behavior proof review",
            &["tokmd", "ripr"],
            "Check whether tests distinguish old from new behavior and prove the PR claim.",
        ),
        model_lane(
            "tests-oracle",
            "Test oracle strength review",
            &["tokmd", "ripr"],
            "Look for smoke-only, tautological, reach-only, or non-discriminating assertions.",
        ),
        model_lane(
            "source-route",
            "Public API source-route review",
            &["tokmd", "ast-grep", "ripr"],
            "Trace public API routes, changed helper callers, sibling paths, and PR claim truth.",
        ),
        model_lane(
            "sibling-paths",
            "Sibling helper and parked follow-up review",
            &["tokmd", "ast-grep"],
            "Find related crypto/compression/runtime helper paths and identify what should be parked rather than broadened.",
        ),
        model_lane(
            "architecture",
            "Boundary and smallest-complete-fix review",
            &["tokmd", "unsafe-review"],
            "Check boundary placement, helper shape, scope control, duplication risk, and smallest complete fix.",
        ),
        model_lane(
            "security",
            "UB as exploit primitive review",
            &["tokmd", "unsafe-review"],
            "Assess OOB, UAF, type confusion, info disclosure, DoS, secret material, and exploitability framing.",
        ),
        model_lane(
            "opposition",
            "Strongest substantiated objection review",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Try to disprove the PR across correctness, proof, portability, performance, route truth, and overclaim risk.",
        ),
    ]
}

pub(crate) fn deep_minimax_lanes() -> Vec<LanePlan> {
    let specs = [
        (
            "ub-memory-lifetime",
            "Memory lifetime review",
            "Focus on object lifetime, borrows, ownership transfer, and stale references.",
        ),
        (
            "ub-pointer-length",
            "Pointer and length coupling review",
            "Focus on stale pointer/length pairs, integer truncation, and offset math.",
        ),
        (
            "ub-active-view",
            "Active view region review",
            "Focus on active view vs backing store boundaries after resize/detach/transfer.",
        ),
        (
            "ub-backing-store",
            "Backing store review",
            "Focus on backing-store aliases, snapshots, and mutations after capture.",
        ),
        (
            "ub-worker-handoff",
            "Worker handoff review",
            "Focus on async worker handoff and JS-backed memory crossing threads.",
        ),
        (
            "ub-gc-detach-transfer",
            "GC detach transfer review",
            "Focus on GC, detach, transfer, and protect/unprotect lifetime hazards.",
        ),
        (
            "tests-red-green",
            "Red green proof review",
            "Focus on whether old main fails and patched code passes for the claimed behavior.",
        ),
        (
            "tests-oracle-strength",
            "Oracle strength review",
            "Focus on revealability, propagation, and non-tautological assertions.",
        ),
        (
            "tests-flake-race",
            "Flake and race review",
            "Focus on timing, async, worker, and platform-dependent proof gaps.",
        ),
        (
            "tests-ci-cost",
            "CI cost review",
            "Focus on whether the proof is cheap enough and placed in the right suite.",
        ),
        (
            "source-route-public-api",
            "Public API route review",
            "Focus on public API entrypoints reaching the changed helper.",
        ),
        (
            "source-route-helper-callers",
            "Helper caller review",
            "Focus on all helper callers and variants affected by the change.",
        ),
        (
            "sibling-paths-crypto",
            "Crypto sibling path review",
            "Focus on PBKDF2, scrypt, hashInto, and key material siblings.",
        ),
        (
            "sibling-paths-compression",
            "Compression sibling path review",
            "Focus on zstd/compression helpers and package-second style siblings.",
        ),
        (
            "sibling-paths-runtime",
            "Runtime sibling path review",
            "Focus on runtime string/buffer helpers and related JS/native boundaries.",
        ),
        (
            "architecture-boundary",
            "Boundary architecture review",
            "Focus on boundary placement, helper shape, and invariants.",
        ),
        (
            "architecture-scope",
            "Scope review",
            "Focus on smallest complete fix and what must not broaden in this PR.",
        ),
        (
            "security-exploitability",
            "Exploitability review",
            "Focus on UB as exploit primitive, OOB, UAF, type confusion, and info leak.",
        ),
        (
            "security-secret-material",
            "Secret material review",
            "Focus on crypto/key/secret material exposure and lifetime risks.",
        ),
        (
            "opposition-overclaim",
            "Overclaim opposition review",
            "Focus on the strongest reason the PR is wrong, incomplete, or overclaimed.",
        ),
    ];
    specs
        .into_iter()
        .map(|(id, role, focus)| {
            model_lane(
                id,
                role,
                &["tokmd", "ripr", "unsafe-review", "ast-grep"],
                focus,
            )
        })
        .collect()
}

pub(crate) fn opencode_go_wide_lanes() -> Vec<LanePlan> {
    let mut lanes = standard_minimax_lanes();
    lanes.extend([
        model_lane(
            "sibling-paths-fast",
            "Fast sibling-path candidate generation",
            &["tokmd", "ast-grep"],
            "Generate candidate-only sibling path gaps and parked follow-up risks. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "source-route-fast",
            "Fast source-route candidate generation",
            &["tokmd", "ast-grep"],
            "Generate candidate-only public API route and helper caller gaps. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "test-gap-fast",
            "Fast test-gap candidate generation",
            &["tokmd", "ripr"],
            "Generate candidate-only weak oracle, red/green, and revealability gaps. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "overclaim-fast",
            "Fast overclaim candidate generation",
            &["tokmd", "ripr", "unsafe-review"],
            "Generate candidate-only overclaim and scope risks. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "security-fast",
            "Fast security candidate generation",
            &["tokmd", "unsafe-review"],
            "Generate candidate-only exploitability, info leak, DoS, and secret material risks. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "refute-finding-1",
            "Fast refutation draft",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Draft candidate-only refutations for top suspected findings. Do not request inline posting.",
        ),
        model_lane(
            "refute-finding-2",
            "Fast refutation draft",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Draft candidate-only refutations for alternative suspected findings. Do not request inline posting.",
        ),
        model_lane(
            "refute-finding-3",
            "Fast refutation draft",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Draft candidate-only refutations for weaker suspected findings. Do not request inline posting.",
        ),
        model_lane(
            "summary-pressure",
            "Fast summary pressure test",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Generate candidate-only pressure on the summary decision, residual risk, and parked follow-ups.",
        ),
        model_lane(
            "duplicate-noise-filter",
            "Fast duplicate and noise filter",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Identify likely duplicates, off-diff concerns, and low-confidence noise. Do not request inline posting.",
        ),
    ]);
    lanes
}

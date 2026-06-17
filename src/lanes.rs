//! Review lane definitions, width routing, and lane plan construction
//! (cleanup train step 13, pure code motion). Lanes describe which
//! model review roles run, what evidence they receive, and how the
//! configured width/policy selects the active set. The lane() constructor
//! itself already lives in builtin.rs; this module owns the named lane
//! definitions and the selection logic.

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

pub(crate) fn unsafe_review_sensor_lane() -> LanePlan {
    LanePlan {
        id: "unsafe-review".to_owned(),
        role: "Unsafe-review comment-plan intake".to_owned(),
        model: "unsafe-review".to_owned(),
        model_display: "unsafe-review deterministic sensor".to_owned(),
        receives: vec!["unsafe-review".to_owned()],
        focus: "Route unsafe-review comment-plan entries through the review compiler.".to_owned(),
    }
}

/// pass only reached the plan artifact (PR #344's run proved the gap: plan
/// notes recorded the lanes, the executed set did not contain them).
/// Merge repo lanes into a selected execution lane set: a repo lane replaces
/// a same-id builtin, otherwise appends. Applied after lane-width selection
/// so repo lanes execute at every width and diff class - the first wiring
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
        DiffClass::WorkflowTooling => workflow_tooling_lanes_for_mix(&plan.language_mix),
        DiffClass::DocsOnly | DiffClass::ArtifactOnlySmoke => Vec::new(),
    }
}

pub(crate) fn source_general_lanes() -> Vec<LanePlan> {
    vec![
        model_lane(
            "correctness",
            "Changed behavior correctness review",
            &["tokmd", "ripr", "ast-grep"],
            "Review changed behavior, public API route truth, regression risk, and overclaim/underclaim without source-UB assumptions.",
        ),
        model_lane(
            "tests-red-green",
            "Red/green changed-behavior proof review",
            &["tokmd", "ripr"],
            "Check whether tests distinguish old from new behavior and prove the PR claim.",
        ),
        model_lane(
            "source-route",
            "Public API source-route review",
            &["tokmd", "ast-grep", "ripr"],
            "Trace public API routes, changed helper callers, sibling paths, and PR claim truth.",
        ),
        model_lane(
            "architecture",
            "Boundary and smallest-complete-fix review",
            &["tokmd", "ast-grep"],
            "Check boundary placement, helper shape, scope control, duplication risk, and smallest complete fix.",
        ),
        model_lane(
            "opposition",
            "Strongest substantiated objection review",
            &["tokmd", "ripr", "ast-grep"],
            "Try to disprove the PR across correctness, proof, portability, performance, route truth, and overclaim risk.",
        ),
    ]
}

pub(crate) fn tests_only_lanes() -> Vec<LanePlan> {
    vec![
        model_lane(
            "tests-red-green",
            "Red/green test proof review",
            &["tokmd", "ripr"],
            "Check whether added or changed tests fail on unpatched code and pass on patched code.",
        ),
        model_lane(
            "tests-oracle",
            "Test oracle strength review",
            &["tokmd", "ripr"],
            "Look for smoke-only, tautological, reach-only, flaky, or non-discriminating assertions.",
        ),
        model_lane(
            "proof-request",
            "Focused proof request review",
            &["tokmd", "ripr"],
            "Request only cheap focused proof that would change reviewer confidence.",
        ),
        model_lane(
            "opposition",
            "Strongest test-suite objection review",
            &["tokmd", "ripr"],
            "Try to disprove whether the test change proves the claimed behavior.",
        ),
    ]
}

pub(crate) fn workflow_tooling_lanes() -> Vec<LanePlan> {
    vec![
        model_lane(
            "workflow-permissions",
            "Workflow permissions and token-scope review",
            &["tokmd", "actionlint", "zizmor"],
            "Check workflow permissions, fork safety, pull_request_target absence, checkout credential persistence, and non-blocking auxiliary behavior.",
        ),
        model_lane(
            "workflow-pinning",
            "Action pinning and runner setup review",
            &["tokmd", "actionlint", "zizmor"],
            "Check action pinning, trusted setup boundaries, tool installation posture, and runner assumptions.",
        ),
        model_lane(
            "workflow-proof",
            "Workflow lint and smoke proof review",
            &["tokmd", "actionlint"],
            "Check whether actionlint or focused smoke proof is available and whether missing proof affects trust.",
        ),
        model_lane(
            "workflow-opposition",
            "Strongest workflow/tooling objection review",
            &["tokmd", "actionlint", "zizmor"],
            "Try to disprove the workflow/tooling change across permissions, triggers, pinning, checkout, fork-only behavior, and reviewer-value claims.",
        ),
    ]
}

pub(crate) fn workflow_tooling_lanes_for_mix(language_mix: &LanguageMix) -> Vec<LanePlan> {
    let has_workflow_surface = language_mix
        .surfaces
        .iter()
        .any(|surface| matches!(surface.as_str(), "workflow" | "action"));
    if has_workflow_surface || language_mix.surfaces.is_empty() {
        return workflow_tooling_lanes();
    }
    tooling_support_lanes()
}

pub(crate) fn tooling_support_lanes() -> Vec<LanePlan> {
    vec![
        model_lane(
            "tooling-script-proof",
            "Tooling script proof review",
            &["tokmd", "ripr", "ast-grep"],
            "Check changed scripts, config, or repo tooling behavior, command compatibility, focused self-test proof, and fixture coverage.",
        ),
        model_lane(
            "tooling-policy",
            "Repo tooling policy review",
            &["tokmd", "cargo-allow", "ast-grep"],
            "Check whether tooling changes preserve repo policy, exception ledgers, generated-artifact boundaries, and setup instructions.",
        ),
        model_lane(
            "tooling-opposition",
            "Strongest tooling objection review",
            &["tokmd", "ripr", "ast-grep"],
            "Try to disprove the tooling change across portability, path handling, command behavior, missing proof, and reviewer-value claims.",
        ),
    ]
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

pub(crate) fn model_lane(id: &str, role: &str, receives: &[&str], focus: &str) -> LanePlan {
    lane(
        id,
        role,
        "custom:MiniMax-M3-3",
        "MiniMax-M3",
        receives,
        focus,
    )
}

pub(crate) fn proof_planner_lane() -> LanePlan {
    model_lane(
        "proof-planner",
        "Model-assisted CI proof planning",
        &[
            "shared-context",
            "diff",
            "proof-requests",
            "runtime-profile",
            "box-state",
        ],
        "Choose only additional cheap proof that would change the review decision. Emit observations, failed_objections, and proof_requests only; never emit candidate findings.",
    )
}

pub(crate) fn follow_up_provider_lane() -> LanePlan {
    model_lane(
        "orchestrator-follow-up",
        "Orchestrator follow-up",
        &[
            "shared-context",
            "work-queue",
            "routed-evidence",
            "follow-up-question",
        ],
        "Classify routed follow-up evidence without posting, mutating, or running commands.",
    )
}

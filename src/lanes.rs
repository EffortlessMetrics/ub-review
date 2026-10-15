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
    specialize_native_lanes_for_language(&mut lanes, plan.diff_class, &plan.language_mix);
    merge_repo_lanes_into(&mut lanes, &plan.repo_lanes);
    lanes
}

/// Keep the existing native-safety roster and sensor routes, but ask questions
/// about Rust contracts when Rust is the only known changed language. Unknown
/// and mixed-language plans retain the full native-runtime focus. Apply this
/// before merging repo lanes so explicit repository contracts remain authoritative.
pub(crate) fn specialize_native_lanes_for_language(
    lanes: &mut [LanePlan],
    diff_class: DiffClass,
    language_mix: &LanguageMix,
) {
    if diff_class != DiffClass::SourceUb
        || !matches!(language_mix.languages.as_slice(), [language] if language == "rust")
    {
        return;
    }
    for lane in lanes {
        let (role, focus) = match lane.id.as_str() {
            "ub" => (
                "Rust native-boundary undefined-behavior review",
                "Review changed Rust pointer provenance, allocation bounds, initialization, aliasing, ownership, drop order, FFI safety contracts, integer truncation, signedness, and overflow. Follow foreign-runtime invalidation only when the changed route exposes it; require a concrete unsafe or FFI boundary before claiming undefined behavior.",
            ),
            "ub-active-view" => (
                "Rust slice extent and allocation-boundary review",
                "Check Rust slice lengths, allocation capacity, offsets, initialization, and pointer validity across reallocation or FFI calls. Tie concerns to changed routes and explicit safety contracts.",
            ),
            "ub-backing-store" => (
                "Rust allocation and aliasing review",
                "Check shared backing allocations, Rust borrow and aliasing invariants, owned snapshots, and mutation after capture at changed unsafe or FFI boundaries.",
            ),
            "ub-worker-handoff" => (
                "Rust thread and async ownership handoff review",
                "Check Rust ownership and captured references across threads, async suspension, cancellation, and FFI callbacks; verify Send/Sync, synchronization, and lifetime contracts on changed routes.",
            ),
            "ub-gc-detach-transfer" => (
                "Rust drop and foreign-lifetime boundary review",
                "Check Rust drop order, allocation ownership transfer, foreign-runtime invalidation, callback lifetimes, and double-free or use-after-free risks at changed unsafe or FFI boundaries.",
            ),
            "sibling-paths" => (
                "Rust sibling helper and caller review",
                "Trace changed Rust helper callers, public API variants, and sibling implementations; bound any proposed follow-up to a concrete affected route.",
            ),
            "sibling-paths-crypto" => (
                "Rust sibling input and output contract review",
                "Check sibling Rust entrypoints for equivalent input/output, ownership, error, and secret-material contracts when the changed API affects them.",
            ),
            "sibling-paths-compression" => (
                "Rust sibling representation and codec review",
                "Check related Rust serialization, parsing, codec, or buffer helpers only when they share the changed representation or boundary.",
            ),
            "sibling-paths-runtime" => (
                "Rust runtime and FFI sibling review",
                "Trace changed Rust runtime, string/buffer, and FFI helper callers and ownership contracts; identify concrete affected siblings before proposing follow-ups.",
            ),
            _ => continue,
        };
        lane.role = role.to_owned();
        lane.focus = focus.to_owned();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn native_plan(files: &[&str], patch: &str) -> Plan {
        let mut plan = crate::tests::test_plan(Vec::new());
        plan.changed_files = files.iter().map(|path| (*path).to_owned()).collect();
        let flags = classify_diff(&plan.changed_files, patch);
        plan.diff_class = classify_diff_class(&plan.changed_files, &flags);
        plan.language_mix = classify_language_mix(&plan.changed_files);
        plan.lanes = crate::diff_posture::default_lanes_for_diff_context(
            plan.diff_class,
            &plan.language_mix,
        );
        plan
    }

    fn assert_same_roster(actual: &[LanePlan], expected: &[LanePlan]) {
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual.id, expected.id);
            assert_eq!(actual.model, expected.model);
            assert_eq!(actual.model_display, expected.model_display);
            assert_eq!(actual.receives, expected.receives);
            if ![
                "ub",
                "ub-active-view",
                "ub-backing-store",
                "ub-worker-handoff",
                "ub-gc-detach-transfer",
                "sibling-paths",
                "sibling-paths-crypto",
                "sibling-paths-compression",
                "sibling-paths-runtime",
            ]
            .contains(&actual.id.as_str())
            {
                assert_eq!(actual.role, expected.role);
                assert_eq!(actual.focus, expected.focus);
            }
        }
    }

    fn assert_rust_focus(lanes: &[LanePlan]) {
        for lane in lanes {
            for unrelated in [
                "ArrayBuffer",
                "TypedArray",
                "JS-backed",
                "JSC",
                "GC",
                "JS/native",
                "PBKDF2",
                "hashInto",
                "package-second",
            ] {
                assert!(
                    !lane.focus.contains(unrelated) && !lane.role.contains(unrelated),
                    "{} retains unrelated {unrelated} axis: {}",
                    lane.id,
                    lane.focus
                );
            }
        }
        assert!(
            lanes
                .iter()
                .any(|lane| lane.receives.iter().any(|id| id == "unsafe-review"))
        );
        assert!(
            lanes
                .iter()
                .any(|lane| lane.receives.iter().any(|id| id == "ripr"))
        );
    }

    #[test]
    fn rust_native_lanes_specialize_focus_without_changing_rosters() {
        let plan = native_plan(&["src/lib.rs"], "+ let p = bytes.as_ptr();");
        assert_eq!(plan.diff_class, DiffClass::SourceUb);
        let mut args = crate::tests::test_run_args(PathBuf::from("unused"));
        for width in [10, 20, 6] {
            args.lane_width = width;
            let lanes = review_lanes_for_args(&plan, &args);
            let legacy = match width {
                10 => standard_minimax_lanes(),
                20 => deep_minimax_lanes(),
                _ => default_lanes(),
            };
            assert_same_roster(&lanes, &legacy);
            assert_rust_focus(&lanes);
        }
        args.lane_width = 20;
        args.provider_policy = ModelProviderPolicy::OpencodeGoWide;
        let lanes = review_lanes_for_args(&plan, &args);
        assert_same_roster(&lanes, &opencode_go_wide_lanes());
        assert_rust_focus(&lanes);
    }

    #[test]
    fn rust_native_default_plan_has_specialized_focus() {
        let plan = native_plan(&["src/lib.rs"], "+ let p = bytes.as_ptr();");
        assert_same_roster(&plan.lanes, &default_lanes());
        assert_rust_focus(&plan.lanes);
        assert_eq!(
            crate::diff_posture::diff_class_posture_heading(plan.diff_class),
            "Native safety"
        );
    }

    #[test]
    fn rust_native_lanes_preserve_distinct_safety_contracts() {
        let plan = native_plan(&["src/ffi.rs"], "+ unsafe boundary();");
        let mut lanes = plan.lanes.clone();
        let mut args = crate::tests::test_run_args(PathBuf::from("unused"));
        for width in [6, 10, 20] {
            args.lane_width = width;
            lanes.extend(review_lanes_for_args(&plan, &args));
        }
        for (id, role_contract, focus_contracts) in [
            (
                "ub",
                "native-boundary",
                ["pointer provenance", "aliasing", "drop order", "FFI"],
            ),
            (
                "ub-active-view",
                "slice extent",
                [
                    "slice lengths",
                    "allocation capacity",
                    "offsets",
                    "pointer validity",
                ],
            ),
            (
                "ub-backing-store",
                "allocation and aliasing",
                [
                    "backing allocations",
                    "borrow",
                    "owned snapshots",
                    "mutation after capture",
                ],
            ),
            (
                "ub-worker-handoff",
                "thread and async",
                [
                    "async suspension",
                    "cancellation",
                    "FFI callbacks",
                    "Send/Sync",
                ],
            ),
            (
                "ub-gc-detach-transfer",
                "drop and foreign-lifetime",
                [
                    "drop order",
                    "ownership transfer",
                    "callback lifetimes",
                    "double-free",
                ],
            ),
            (
                "sibling-paths",
                "helper and caller",
                [
                    "helper callers",
                    "public API variants",
                    "sibling implementations",
                    "concrete affected route",
                ],
            ),
            (
                "sibling-paths-crypto",
                "input and output",
                ["input/output", "ownership", "error", "secret-material"],
            ),
            (
                "sibling-paths-compression",
                "representation and codec",
                [
                    "serialization",
                    "parsing",
                    "codec",
                    "changed representation",
                ],
            ),
            (
                "sibling-paths-runtime",
                "runtime and FFI",
                [
                    "string/buffer",
                    "FFI helper callers",
                    "ownership contracts",
                    "concrete affected siblings",
                ],
            ),
        ] {
            let matching = lanes
                .iter()
                .filter(|lane| lane.id == id)
                .collect::<Vec<_>>();
            assert!(!matching.is_empty(), "missing safety contract lane {id}");
            for lane in matching {
                assert!(lane.role.starts_with("Rust "), "{id}: {}", lane.role);
                assert!(lane.role.contains(role_contract), "{id}: {}", lane.role);
                for contract in focus_contracts {
                    assert!(
                        lane.focus.contains(contract),
                        "{id} lacks {contract}: {}",
                        lane.focus
                    );
                }
            }
        }
    }

    #[test]
    fn rust_ffi_retains_native_boundary_review() -> Result<()> {
        let plan = native_plan(
            &["src/ffi.rs"],
            "+ unsafe extern \"C\" { fn consume(p: *mut u8); }",
        );
        assert_eq!(plan.diff_class, DiffClass::SourceUb);
        let args = crate::tests::test_run_args(PathBuf::from("unused"));
        let lanes = review_lanes_for_args(&plan, &args);
        for id in ["ub-active-view", "ub-worker-handoff"] {
            let lane = lanes
                .iter()
                .find(|lane| lane.id == id)
                .with_context(|| format!("missing native safety lane {id}"))?;
            assert!(lane.focus.contains("FFI"));
        }
        for id in [
            "ub-memory-lifetime",
            "tests-red-green",
            "tests-oracle",
            "source-route",
            "security",
            "opposition",
        ] {
            assert!(lanes.iter().any(|lane| lane.id == id));
        }
        assert_rust_focus(&lanes);
        Ok(())
    }

    #[test]
    fn bun_mixed_and_unknown_languages_keep_existing_focus() -> Result<()> {
        for files in [
            vec!["src/bun.js/bindings/ArrayBuffer.zig"],
            vec!["src/lib.rs", "src/native.cpp"],
            vec!["src/lib.rs", "src/bridge.zig", "src/worker.js"],
        ] {
            let plan = native_plan(&files, "+ unsafe boundary();");
            let mut args = crate::tests::test_run_args(PathBuf::from("unused"));
            for width in [6, 10, 20] {
                args.lane_width = width;
                assert_eq!(
                    serde_json::to_value(review_lanes_for_args(&plan, &args))?,
                    serde_json::to_value(review_lanes_for_width(width, &plan))?
                );
            }
            assert_eq!(
                serde_json::to_value(&plan.lanes)?,
                serde_json::to_value(default_lanes())?
            );
        }
        let mut plan = native_plan(&["src/lib.rs"], "+ unsafe boundary();");
        plan.language_mix = LanguageMix::default();
        plan.lanes = default_lanes();
        let args = crate::tests::test_run_args(PathBuf::from("unused"));
        assert_eq!(
            serde_json::to_value(review_lanes_for_args(&plan, &args))?,
            serde_json::to_value(standard_minimax_lanes())?
        );
        Ok(())
    }

    #[test]
    fn rust_native_selectors_and_repo_overrides_remain_authoritative() -> Result<()> {
        let mut plan = native_plan(&["src/lib.rs"], "+ let p = bytes.as_ptr();");
        let mut args = crate::tests::test_run_args(PathBuf::from("unused"));
        args.selectors.lanes = "ub-active-view,ub-worker-handoff".to_owned();
        args.selectors.except_lanes = "ub-worker-handoff".to_owned();
        let selected = selected_review_lanes_for_args(&plan, &args)?;
        assert_eq!(
            selected
                .iter()
                .map(|lane| lane.id.as_str())
                .collect::<Vec<_>>(),
            ["ub-active-view"]
        );
        assert_rust_focus(&selected_review_lanes_for_args(
            &plan,
            &crate::tests::test_run_args(PathBuf::from("unused")),
        )?);
        args.selectors.lanes = "missing-lane".to_owned();
        assert!(selected_review_lanes_for_args(&plan, &args).is_err());

        let mut override_lane = model_lane(
            "ub-active-view",
            "Repo interop review",
            &["unsafe-review"],
            "Repo-owned ArrayBuffer contract.",
        );
        override_lane.model = "custom:repo-model".to_owned();
        plan.repo_lanes.push(override_lane.clone());
        plan.repo_lanes.push(model_lane(
            "repo-extra",
            "Repo route",
            &["ripr"],
            "Repo-owned route.",
        ));
        args.selectors.lanes = "ub-active-view,repo-extra".to_owned();
        let selected = selected_review_lanes_for_args(&plan, &args)?;
        assert_eq!(selected.len(), 2);
        assert_eq!(
            serde_json::to_value(&selected[0])?,
            serde_json::to_value(override_lane)?
        );
        assert_eq!(selected[1].id, "repo-extra");
        Ok(())
    }

    #[test]
    fn ordinary_rust_keeps_source_general_coverage() -> Result<()> {
        let plan = native_plan(&["src/lib.rs"], "+ let count = count + 1;");
        assert_eq!(plan.diff_class, DiffClass::SourceGeneral);
        let args = crate::tests::test_run_args(PathBuf::from("unused"));
        assert_eq!(
            serde_json::to_value(review_lanes_for_args(&plan, &args))?,
            serde_json::to_value(source_general_lanes())?
        );
        Ok(())
    }
}

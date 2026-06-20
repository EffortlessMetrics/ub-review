//! Diff class posture: default lane sets per diff class and review
//! posture headings (cleanup train step 56, pure code motion).

use crate::*;

pub(crate) fn default_lanes_for_diff_context(
    diff_class: DiffClass,
    language_mix: &LanguageMix,
) -> Vec<LanePlan> {
    match diff_class {
        DiffClass::SourceUb => default_lanes(),
        DiffClass::SourceGeneral => source_general_lanes(),
        DiffClass::TestsOnly => tests_only_lanes(),
        DiffClass::WorkflowTooling => workflow_tooling_lanes_for_mix(language_mix),
        DiffClass::DocsOnly | DiffClass::ArtifactOnlySmoke => Vec::new(),
    }
}

pub(crate) const NO_LGTM_POSTURE: &str = r#"Standalone approval language is banned.

Do not answer with only a one-word approval, a generic quality adjective, or a zero-actionable shorthand.

A zero-finding review is not approval. It must report:
1. what concrete paths, invariants, tests, or claims were checked;
2. the strongest failed objection;
3. why that objection did not hold;
4. residual risk for a human to verify.
"#;

pub(crate) const WORKFLOW_TOOLING_POSTURE: &str = r#"Standalone approval language is banned.

Return workflow/tooling reviewer value only: findings, verification questions, actionlint/zizmor proof results, refutations, residual workflow risk, parked follow-ups, and trust-affecting missing workflow evidence.

Check permissions, trigger safety, action pinning, checkout credential persistence, fork-only behavior, pull_request_target absence, auxiliary/non-blocking semantics, and actionlint availability.

Do not add ArrayBuffer, worker-handoff, native UB, or source-route narrative unless the diff actually touches those paths.
"#;

pub(crate) const SOURCE_GENERAL_POSTURE: &str = r#"Standalone approval language is banned.

Return changed-behavior reviewer value only: findings, verification questions, proof results, refutations, residual risk, parked follow-ups, and trust-affecting missing evidence.

Check route truth, test proof, overclaims, behavior regressions, performance risk, and smallest-complete-fix boundaries.

Do not add native UB, ArrayBuffer, or worker-handoff narrative unless the diff actually touches those paths.
"#;

pub(crate) const TESTS_ONLY_POSTURE: &str = r#"Standalone approval language is banned.

Return test-review value only: test-oracle gaps, red/green proof questions, proof results, refutations, residual test risk, parked follow-ups, and trust-affecting missing proof evidence.

Check whether tests discriminate the patch, whether assertions are non-tautological, whether focused proof is cheap, and whether missing base+tests evidence affects trust.

Do not add source UB, ArrayBuffer, worker-handoff, or source-route narrative unless the diff actually touches those paths.
"#;

pub(crate) const DOCS_ONLY_POSTURE: &str = r#"Standalone approval language is banned.

Return documentation reviewer value only: factual issues, verification questions, refutations, residual documentation risk, parked follow-ups, and trust-affecting missing evidence.

Check claim accuracy, links, examples, release/process promises, and whether docs overstate unproven behavior.

Do not add source UB, ArrayBuffer, worker-handoff, workflow, or test-proof narrative unless the diff actually touches those paths.
"#;

pub(crate) fn diff_class_posture_heading(diff_class: DiffClass) -> &'static str {
    match diff_class {
        DiffClass::SourceUb => "Bun UB",
        DiffClass::SourceGeneral => "Source-general",
        DiffClass::TestsOnly => "Tests-only",
        DiffClass::WorkflowTooling => "Workflow/tooling",
        DiffClass::DocsOnly => "Docs-only",
        DiffClass::ArtifactOnlySmoke => "Artifact-only smoke",
    }
}

pub(crate) fn review_posture_for_diff_class(diff_class: DiffClass) -> &'static str {
    match diff_class {
        DiffClass::SourceUb => NO_LGTM_POSTURE,
        DiffClass::SourceGeneral => SOURCE_GENERAL_POSTURE,
        DiffClass::TestsOnly => TESTS_ONLY_POSTURE,
        DiffClass::WorkflowTooling => WORKFLOW_TOOLING_POSTURE,
        DiffClass::DocsOnly | DiffClass::ArtifactOnlySmoke => DOCS_ONLY_POSTURE,
    }
}

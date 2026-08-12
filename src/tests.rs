//! Byte-exact fixtures for compiled review pre-delivery payload values.
//!
//! Each case runs through the production review compiler. Inline text
//! then runs through `github_review_post_comment_body`, the same delivery
//! transform that strips lane identity and renders suggestion fences. The
//! checked-in files stop before network delivery, which adds request metadata.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use crate::*;

const BASE: &str = "origin/main";
const HEAD: &str = "HEAD";
const SHARED_CONTEXT_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct GoldenCase {
    id: &'static str,
    shape: &'static str,
    diff_class: DiffClass,
    changed_files: Vec<String>,
    patch: String,
    inline_comments: Vec<ReviewInlineComment>,
    summary_only_findings: Vec<SummaryOnlyFinding>,
    observations: Vec<Observation>,
    proof_receipts: Vec<ProofReceipt>,
}

impl GoldenCase {
    fn compile(&self) -> Result<CompiledReviewSurface> {
        let args = crate::tests::test_run_args(PathBuf::from("target/review-golden"));
        let mut plan = crate::tests::test_plan(Vec::new());
        plan.base = BASE.to_owned();
        plan.head = HEAD.to_owned();
        plan.diff_class = self.diff_class;
        plan.changed_files = self.changed_files.clone();
        plan.language_mix = classify_language_mix(&self.changed_files);
        plan.sensors.clear();
        plan.lanes.clear();
        plan.repo_lanes.clear();
        plan.docs_only = matches!(self.diff_class, DiffClass::DocsOnly);
        plan.notes.clear();

        let diff = DiffContext {
            base: BASE.to_owned(),
            head: HEAD.to_owned(),
            changed_files: self.changed_files.clone(),
            flags: classify_diff(&self.changed_files, &self.patch),
            patch: self.patch.clone(),
            diff_class: self.diff_class,
        };
        let body_policy = ReviewBodyPolicy::default();
        compile_review_surface(ReviewCompilerInput {
            shared_context_id: SHARED_CONTEXT_ID,
            review_body_policy: &body_policy,
            run_pass: RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &self.inline_comments,
            summary_only_findings: &self.summary_only_findings,
            observations: &self.observations,
            proof_receipts: &self.proof_receipts,
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })
    }
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/review-golden")
}

fn bless_enabled() -> bool {
    bless_value_enabled(std::env::var("UB_REVIEW_BLESS").ok().as_deref())
}

fn bless_value_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

fn snapshot_text(case: &GoldenCase, surface: &CompiledReviewSurface) -> Result<String> {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/review-golden");
    fs::create_dir_all(&out).with_context(|| format!("create {}", out.display()))?;
    let patch_path = out.join(format!("{}.patch", case.id));
    fs::write(&patch_path, &case.patch)
        .with_context(|| format!("write {}", patch_path.display()))?;
    let post_args = PostArgs {
        review_json: out.join(format!("{}.json", case.id)),
        diff_patch: Some(patch_path),
        out,
        github_token: None,
        repo: None,
        pull_number: None,
        github_api_url: "https://api.github.invalid".to_owned(),
        fail_on_post_error: true,
    };
    if surface.should_prepare_github_review {
        validate_github_review_payload_for_post(&post_args, &surface.github_review)
            .with_context(|| format!("validate `{}` post payload", case.id))?;
    }
    let post_payload = github_review_post_payload(&surface.github_review)?;
    let mut text = String::new();
    text.push_str(&format!("case: {}\n", case.id));
    text.push_str(&format!("diff shape: {}\n", case.shape));
    text.push_str(&format!(
        "review payload status: {}\n",
        surface.review_payload_status
    ));
    text.push_str(&format!(
        "posts a review: {}\n",
        surface.should_prepare_github_review
    ));
    text.push_str(&format!("event: {}\n", surface.github_review.event));
    text.push_str(&format!(
        "inline comments: {}\n",
        surface.github_review.comments.len()
    ));

    text.push_str("\n=== pr review body ===\n");
    if surface.github_review.body.is_empty() {
        text.push_str("(empty: nothing is posted to the pull request)\n");
    } else {
        text.push_str(&surface.github_review.body);
        if !surface.github_review.body.ends_with('\n') {
            text.push('\n');
        }
    }
    text.push_str("=== end pr review body ===\n");

    for (index, comment) in post_payload.comments.iter().enumerate() {
        text.push_str(&format!("\n=== inline comment {} ===\n", index + 1));
        text.push_str(&format!(
            "{}:{} {}\n",
            comment.path, comment.line, comment.side
        ));
        text.push_str(&comment.body);
        if !comment.body.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("=== end inline comment {} ===\n", index + 1));
    }
    text.push_str("\n=== serialized pre-delivery review payload ===\n");
    text.push_str(&serde_json::to_string_pretty(&post_payload)?);
    text.push_str("\n=== end serialized pre-delivery review payload ===\n");
    Ok(text)
}

fn assert_golden(case: &GoldenCase, actual: &str) -> Result<()> {
    assert_golden_with_bless(case, actual, bless_enabled())
}

fn assert_golden_with_bless(case: &GoldenCase, actual: &str, bless: bool) -> Result<()> {
    let dir = golden_dir();
    let path = dir.join(format!("{}.txt", case.id));
    if bless {
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        fs::write(&path, actual).with_context(|| format!("write {}", path.display()))?;
        return Ok(());
    }
    let expected = fs::read_to_string(&path).with_context(|| {
        format!(
            "read {}; refresh with UB_REVIEW_BLESS=1 cargo test --locked --bin ub-review review_golden",
            path.display()
        )
    })?;
    ensure!(
        expected == actual,
        "GitHub-facing review changed for `{}`.\n--- expected ({}) ---\n{}\n--- actual ---\n{}\n--- end ---",
        case.id,
        path.display(),
        expected,
        actual
    );
    Ok(())
}

fn rust_patch() -> String {
    "diff --git a/src/buffer.rs b/src/buffer.rs\n\
     --- a/src/buffer.rs\n\
     +++ b/src/buffer.rs\n\
     @@ -38,0 +39,3 @@\n\
     +    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };\n\
     +    if len == 0 {\n\
     +        return &[];\n"
        .to_owned()
}

fn test_patch() -> String {
    "diff --git a/tests/buffer.rs b/tests/buffer.rs\n\
     --- a/tests/buffer.rs\n\
     +++ b/tests/buffer.rs\n\
     @@ -9,0 +10,3 @@\n\
     +#[test]\n\
     +fn empty_buffer_returns_empty_slice() {\n\
     +    assert!(buffer.as_slice().is_empty());\n"
        .to_owned()
}

fn inline_comment(body: &str, suggestion: Option<&str>) -> ReviewInlineComment {
    ReviewInlineComment {
        lane: "ub".to_owned(),
        severity: "high".to_owned(),
        confidence: "high".to_owned(),
        path: "src/buffer.rs".to_owned(),
        line: 39,
        side: "RIGHT".to_owned(),
        body: format!("[ub] {body}"),
        evidence: "src/buffer.rs:39-41 and the changed call order".to_owned(),
        suggestion: suggestion.map(ToOwned::to_owned),
    }
}

fn summary_finding(reason: &str) -> SummaryOnlyFinding {
    SummaryOnlyFinding {
        lane: "source-route".to_owned(),
        severity: "medium".to_owned(),
        confidence: "high".to_owned(),
        reason: reason.to_owned(),
        evidence: "bounded sibling-route inspection".to_owned(),
    }
}

fn observation(claim: &str) -> Observation {
    make_observation(ObservationInput {
        index: 0,
        lane: "tests-oracle",
        question: "Does the changed test distinguish this patch?",
        claim,
        kind: "missing-evidence",
        status: "open",
        severity: "high",
        confidence: "high",
        path: None,
        line: None,
        evidence: vec!["focused red/green receipt".to_owned()],
        dedupe_key: None,
        source: "model-observation",
    })
}

fn red_green_receipt(
    id: &str,
    result: &str,
    base_status: &str,
    request_ids: Vec<String>,
) -> ProofReceipt {
    ProofReceipt {
        schema: "ub-review.proof_receipt.v1".to_owned(),
        id: id.to_owned(),
        kind: "focused-red-green".to_owned(),
        base: BASE.to_owned(),
        head: HEAD.to_owned(),
        test_patch_mode: "base-plus-tests".to_owned(),
        requested_by: vec!["tests-oracle".to_owned()],
        request_ids,
        commands: vec![
            ProofCommandReceipt {
                side: "head".to_owned(),
                command: "cargo test --locked empty_buffer_returns_empty_slice".to_owned(),
                env: BTreeMap::new(),
                status: "passed".to_owned(),
                exit_code: Some(0),
                timed_out: false,
                timeout_sec: 300,
                duration_ms: 42,
                stdout: format!("proof/{id}/head/stdout.txt"),
                stderr: format!("proof/{id}/head/stderr.txt"),
                reason: "focused test executed on HEAD".to_owned(),
            },
            ProofCommandReceipt {
                side: "base-plus-tests".to_owned(),
                command: "cargo test --locked empty_buffer_returns_empty_slice".to_owned(),
                env: BTreeMap::new(),
                status: base_status.to_owned(),
                exit_code: Some(if base_status == "passed" { 0 } else { 1 }),
                timed_out: false,
                timeout_sec: 300,
                duration_ms: 41,
                stdout: format!("proof/{id}/base-plus-tests/stdout.txt"),
                stderr: format!("proof/{id}/base-plus-tests/stderr.txt"),
                reason: "focused test executed on base plus changed tests".to_owned(),
            },
        ],
        result: result.to_owned(),
        reason: if result == "discriminating" {
            "HEAD passed; base+tests failed".to_owned()
        } else {
            "HEAD and base+tests both passed".to_owned()
        },
    }
}

fn clean_case() -> GoldenCase {
    GoldenCase {
        id: "clean-no-findings",
        shape: "small Rust refactor with nothing useful to say",
        diff_class: DiffClass::SourceGeneral,
        changed_files: vec!["src/parser.rs".to_owned()],
        patch: "diff --git a/src/parser.rs b/src/parser.rs\n--- a/src/parser.rs\n+++ b/src/parser.rs\n@@ -12 +12 @@\n-let n = cap as usize;\n+let n = usize::from(cap);\n".to_owned(),
        inline_comments: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: Vec::new(),
        proof_receipts: Vec::new(),
    }
}

fn one_inline_case() -> GoldenCase {
    GoldenCase {
        id: "one-inline-finding",
        shape: "one source-local unsafe ordering defect",
        diff_class: DiffClass::SourceUb,
        changed_files: vec!["src/buffer.rs".to_owned()],
        patch: rust_patch(),
        inline_comments: vec![inline_comment(
            "`from_raw_parts` runs before the zero-length guard, so a dangling pointer is used even though the slice is never read.",
            None,
        )],
        summary_only_findings: Vec::new(),
        observations: Vec::new(),
        proof_receipts: Vec::new(),
    }
}

fn inline_and_summary_case() -> GoldenCase {
    GoldenCase {
        id: "inline-and-summary-finding",
        shape: "one anchored defect plus one cross-cutting route concern",
        diff_class: DiffClass::SourceUb,
        changed_files: vec!["src/buffer.rs".to_owned(), "src/pool.rs".to_owned()],
        patch: rust_patch(),
        inline_comments: vec![inline_comment(
            "`from_raw_parts` runs before the zero-length guard, so a dangling pointer is used even though the slice is never read.",
            None,
        )],
        summary_only_findings: vec![summary_finding(
            "`Pool::lease` retains the same construction outside this diff, so the changed fix does not cover every production route.",
        )],
        observations: Vec::new(),
        proof_receipts: Vec::new(),
    }
}

fn evidence_gap_case() -> GoldenCase {
    let observation = observation(
        "The changed test passes on both HEAD and base+tests, so it does not establish the patch-specific behavior.",
    );
    let receipt = red_green_receipt(
        "proof-golden-non-discriminating",
        "non_discriminating",
        "passed",
        vec![observation.id.clone(), observation.dedupe_key.clone()],
    );
    GoldenCase {
        id: "evidence-gap",
        shape: "focused red/green proof that does not discriminate",
        diff_class: DiffClass::SourceUb,
        changed_files: vec!["src/buffer.rs".to_owned(), "tests/buffer.rs".to_owned()],
        patch: format!("{}{}", rust_patch(), test_patch()),
        inline_comments: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: vec![observation],
        proof_receipts: vec![receipt],
    }
}

fn discriminating_proof_case() -> GoldenCase {
    GoldenCase {
        id: "test-proof-and-verification",
        shape: "focused test passes on HEAD and fails on base plus changed tests",
        diff_class: DiffClass::SourceUb,
        changed_files: vec!["src/buffer.rs".to_owned(), "tests/buffer.rs".to_owned()],
        patch: format!("{}{}", rust_patch(), test_patch()),
        inline_comments: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: Vec::new(),
        proof_receipts: vec![red_green_receipt(
            "proof-golden-discriminating",
            "discriminating",
            "failed",
            vec!["req-golden-discriminating".to_owned()],
        )],
    }
}

fn suggestion_case() -> GoldenCase {
    GoldenCase {
        id: "inline-suggestion",
        shape: "one exact source-local repair suggestion",
        diff_class: DiffClass::SourceUb,
        changed_files: vec!["src/buffer.rs".to_owned()],
        patch: rust_patch(),
        inline_comments: vec![ReviewInlineComment {
            lane: "unsafe-review".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/buffer.rs".to_owned(),
            line: 39,
            side: "RIGHT".to_owned(),
            body: "[unsafe-review] Avoid constructing a slice from the pointer when the length is zero.".to_owned(),
            evidence: "src/buffer.rs:39 and the changed call order".to_owned(),
            suggestion: Some(
                "let slice = if len == 0 { &[] } else { unsafe { core::slice::from_raw_parts(ptr, len) } };".to_owned(),
            ),
        }],
        summary_only_findings: Vec::new(),
        observations: Vec::new(),
        proof_receipts: Vec::new(),
    }
}

fn cases() -> Vec<GoldenCase> {
    vec![
        clean_case(),
        one_inline_case(),
        inline_and_summary_case(),
        evidence_gap_case(),
        discriminating_proof_case(),
        suggestion_case(),
    ]
}

#[test]
fn review_goldens_match_the_exact_github_facing_surface() -> Result<()> {
    for case in cases() {
        let surface = case.compile()?;
        let actual = snapshot_text(&case, &surface)?;
        assert_golden(&case, &actual)?;
    }
    Ok(())
}

#[test]
fn proof_cases_use_production_red_green_receipts() {
    for (case, expected_result, expected_base_status, expected_base_exit) in [
        (evidence_gap_case(), "non_discriminating", "passed", 0),
        (discriminating_proof_case(), "discriminating", "failed", 1),
    ] {
        let receipt = &case.proof_receipts[0];
        assert_eq!(receipt.kind, "focused-red-green");
        assert_eq!(receipt.test_patch_mode, "base-plus-tests");
        assert_eq!(receipt.result, expected_result);
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].side, "head");
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[0].exit_code, Some(0));
        assert!(!receipt.commands[0].timed_out);
        assert_eq!(receipt.commands[1].side, "base-plus-tests");
        assert_eq!(receipt.commands[1].status, expected_base_status);
        assert_eq!(receipt.commands[1].exit_code, Some(expected_base_exit));
        assert!(!receipt.commands[1].timed_out);
    }
}

#[test]
fn golden_bless_is_fail_closed_without_exact_opt_in() {
    for value in [None, Some(""), Some("0"), Some("true"), Some("01")] {
        assert!(!bless_value_enabled(value));
    }
    assert!(bless_value_enabled(Some("1")));
}

#[test]
fn missing_golden_reports_exact_path_and_refresh_command() -> Result<()> {
    let mut case = clean_case();
    case.id = "missing-golden-proof";
    let error = match assert_golden_with_bless(&case, "unused", false) {
        Ok(()) => anyhow::bail!("missing golden unexpectedly succeeded"),
        Err(error) => error,
    };
    let message = format!("{error:#}");
    ensure!(message.contains("missing-golden-proof.txt"), "{message}");
    ensure!(
        message.contains("UB_REVIEW_BLESS=1 cargo test"),
        "{message}"
    );
    Ok(())
}

#[test]
fn snapshot_uses_the_production_inline_delivery_transform() -> Result<()> {
    let case = suggestion_case();
    let surface = case.compile()?;
    ensure!(surface.github_review.comments.len() == 1);

    let actual = snapshot_text(&case, &surface)?;
    ensure!(!actual.contains("[ub]"));
    ensure!(actual.contains(
        "```suggestion\nlet slice = if len == 0 { &[] } else { unsafe { core::slice::from_raw_parts(ptr, len) } };\n```"
    ));
    Ok(())
}

#[test]
fn clean_case_stays_silent() -> Result<()> {
    let surface = clean_case().compile()?;
    ensure!(!surface.should_prepare_github_review);
    ensure!(surface.github_review.body.is_empty());
    ensure!(surface.github_review.comments.is_empty());
    Ok(())
}

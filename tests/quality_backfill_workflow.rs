/// Find a required workflow marker so ordering assertions fail with context.
fn required_position(text: &str, needle: &str) -> Result<usize, String> {
    text.find(needle)
        .ok_or_else(|| format!("workflow contract is missing `{needle}`"))
}

#[test]
fn quality_backfill_publishes_only_compact_current_output() -> Result<(), String> {
    let workflow = include_str!("../.github/workflows/quality-backfill.yml");
    let pinned_upload =
        "- uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7";

    for expected in [
        "UB_REVIEW_QUALITY_INPUT_DIR: target/ub-review-quality/source",
        "UB_REVIEW_QUALITY_PUBLISH_DIR: target/ub-review-quality-publish",
        "UB_REVIEW_QUALITY_SOURCE_ARTIFACT_MAX_BYTES: 67108864",
        "python3 scripts/quality-bootstrap.py",
        "--diagnostic target/ub-review-quality-diagnostics/bootstrap.json",
        "rm -rf -- \"$UB_REVIEW_QUALITY_INPUT_DIR/runs\" \"$UB_REVIEW_QUALITY_INPUT_DIR/previous\"",
        "rm -rf -- \"$UB_REVIEW_QUALITY_PUBLISH_DIR\"",
        "--pull-numbers-file target/ub-review-quality/source/github/pr-numbers.txt",
        "--out \"$UB_REVIEW_QUALITY_PUBLISH_DIR\"",
        "--github-outcomes \"$UB_REVIEW_QUALITY_INPUT_DIR/github/github-quality-outcomes.json\"",
        "- name: Download previous quality backfill (bounded)",
        "- name: Verify quality backfill publication tree",
        "test -s \"$UB_REVIEW_QUALITY_PUBLISH_DIR/review/quality-backfill.json\"",
        "-type l -print -quit",
        "UB_REVIEW_QUALITY_MAX_FILES",
        "UB_REVIEW_QUALITY_MAX_FILE_BYTES",
        "UB_REVIEW_QUALITY_MAX_TOTAL_BYTES",
        "if: steps.publish_preflight.outcome == 'success'",
        "path: ${{ env.UB_REVIEW_QUALITY_PUBLISH_DIR }}",
        "if: failure() && steps.github_bootstrap.outcome == 'failure'",
        "name: ub-review-quality-bootstrap-unavailable",
        "path: target/ub-review-quality-diagnostics/bootstrap.json",
        pinned_upload,
    ] {
        assert!(
            workflow.contains(expected),
            "quality backfill workflow is missing `{expected}`"
        );
    }

    for forbidden in [
        "path: target/ub-review-quality\n",
        "--out target/ub-review-quality \\",
        "path: ${{ env.UB_REVIEW_QUALITY_INPUT_DIR }}",
        "- uses: actions/upload-artifact@v7",
        "rm -rf -- \"$UB_REVIEW_QUALITY_INPUT_DIR\"",
        "gh pr list",
    ] {
        assert!(
            !workflow.contains(forbidden),
            "quality backfill workflow restored shared or mutable publication contract `{forbidden}`"
        );
    }

    let preflight =
        required_position(workflow, "- name: Verify quality backfill publication tree")?;
    let upload = required_position(workflow, pinned_upload)?;
    assert!(
        preflight < upload,
        "publication preflight must run before artifact upload"
    );

    let bootstrap = required_position(workflow, "- name: Collect bounded GitHub bootstrap")?;
    let collect = required_position(workflow, "- name: Collect GitHub receipts")?;
    let clear_inputs = required_position(
        workflow,
        "rm -rf -- \"$UB_REVIEW_QUALITY_INPUT_DIR/runs\" \"$UB_REVIEW_QUALITY_INPUT_DIR/previous\"",
    )?;
    let downloads = required_position(workflow, "- name: Download bounded recent gate artifacts")?;
    assert!(bootstrap < collect && collect < clear_inputs && clear_inputs < downloads);

    let previous_download = required_position(
        workflow,
        "- name: Download previous quality backfill (bounded)",
    )?;
    let build = required_position(workflow, "- name: Build compact quality backfill")?;
    assert!(
        previous_download < build,
        "previous backfill is input to the fresh compact build"
    );

    Ok(())
}

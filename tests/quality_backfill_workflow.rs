fn required_position(text: &str, needle: &str) -> Result<usize, String> {
    text.find(needle)
        .ok_or_else(|| format!("workflow contract is missing `{needle}`"))
}

#[test]
fn quality_backfill_publishes_only_compact_current_output() -> Result<(), String> {
    let workflow = include_str!("../.github/workflows/quality-backfill.yml");

    for expected in [
        "UB_REVIEW_QUALITY_INPUT_DIR: ${{ runner.temp }}/ub-review-quality-input",
        "UB_REVIEW_QUALITY_PUBLISH_DIR: target/ub-review-quality-publish",
        "UB_REVIEW_QUALITY_SOURCE_ARTIFACT_MAX_BYTES: 67108864",
        "rm -rf -- \"$UB_REVIEW_QUALITY_INPUT_DIR\"",
        "rm -rf -- \"$UB_REVIEW_QUALITY_PUBLISH_DIR\"",
        "--out \"$UB_REVIEW_QUALITY_PUBLISH_DIR\"",
        "--github-outcomes \"$UB_REVIEW_QUALITY_INPUT_DIR/github/github-quality-outcomes.json\"",
        "- name: Verify quality backfill publication tree",
        "test -s \"$UB_REVIEW_QUALITY_PUBLISH_DIR/review/quality-backfill.json\"",
        "-type l -print -quit",
        "UB_REVIEW_QUALITY_MAX_FILES",
        "UB_REVIEW_QUALITY_MAX_FILE_BYTES",
        "UB_REVIEW_QUALITY_MAX_TOTAL_BYTES",
        "if: steps.publish_preflight.outcome == 'success'",
        "path: ${{ env.UB_REVIEW_QUALITY_PUBLISH_DIR }}",
    ] {
        assert!(
            workflow.contains(expected),
            "quality backfill workflow is missing `{expected}`"
        );
    }

    for forbidden in [
        "target/ub-review-quality/source",
        "path: target/ub-review-quality\n",
        "--out target/ub-review-quality \\",
    ] {
        assert!(
            !workflow.contains(forbidden),
            "quality backfill workflow restored shared source/publish path `{forbidden}`"
        );
    }

    let preflight = required_position(
        workflow,
        "- name: Verify quality backfill publication tree",
    )?;
    let upload = required_position(workflow, "- uses: actions/upload-artifact@v7")?;
    assert!(
        preflight < upload,
        "publication preflight must run before artifact upload"
    );

    let previous_download = required_position(
        workflow,
        "- name: Download bounded previous quality backfill",
    )?;
    let build = required_position(workflow, "- name: Build compact quality backfill")?;
    assert!(
        previous_download < build,
        "previous backfill is input to the fresh compact build"
    );

    Ok(())
}

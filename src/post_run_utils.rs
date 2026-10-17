//! Post-run utilities: model status sections, GitHub step summary,
//! doctor tool helpers, cache/env utilities, and profile config hash
//! (cleanup train step 54, pure code motion).

use crate::*;

pub(crate) fn render_model_status_sections(text: &mut String, out: &Path) {
    text.push_str("\n## Model review status\n\n");
    let Some(review) = read_review_summary_receipt(out) else {
        text.push_str("- Model review: `incomplete`; `review/review.json` is missing, unreadable, or invalid.\n");
        return;
    };
    let selected = reporting_effective_model_lane_ids(out);
    let missing_or_failed = model_status_evidence_issues(&review, selected.as_deref());
    let coverage_complete = review.model_mode == "auto"
        && selected.as_ref().is_some_and(|lanes| {
            !lanes.is_empty()
                && lanes.iter().all(|lane| {
                    review.model_lanes.iter().any(|receipt| {
                        &receipt.lane == lane && model_lane_is_usable_for_terminal_state(receipt)
                    })
                })
        })
        && review
            .model_lanes
            .iter()
            .all(model_lane_is_usable_for_terminal_state);
    let status = if review.model_mode == "off" {
        "`not_attempted`; model mode off supports setup, testing, or fallback; no model review was requested."
    } else if !coverage_complete {
        "`incomplete`; model review is not established for every selected lane."
    } else if missing_or_failed.is_empty() {
        "`completed`; every selected lane has a usable model receipt."
    } else {
        "`limited`; selected lanes have usable model receipts with reported limitations."
    };
    text.push_str(&format!("- Model review: {status}\n"));

    text.push_str("\n## Provider preflights\n\n");
    text.push_str(&format!(
        "- Model mode: `{}`\n- Depth: `{}`\n- Provider policy: `{}`\n- Lane width: `{}`\n\n",
        review.model_mode,
        if review.depth.is_empty() {
            ReviewDepth::Standard.key()
        } else {
            review.depth.as_str()
        },
        review.provider_policy,
        review.lane_width
    ));
    if review.provider_preflights.is_empty() {
        text.push_str("- No provider preflight receipts were produced.\n");
    } else {
        text.push_str("| Provider | Model | Endpoint | Status | HTTP | Response | Reason |\n");
        text.push_str("|---|---|---|---|---|---|---|\n");
        for receipt in &review.provider_preflights {
            text.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` | {} | {} | {} |\n",
                receipt.provider,
                receipt.model,
                receipt.endpoint_kind,
                receipt.status,
                optional_u16_cell(receipt.http_status),
                optional_str_cell(receipt.response_shape.as_deref()),
                escape_md(&receipt.reason)
            ));
        }
    }

    text.push_str("\n## Model lane status\n\n");
    if review.model_lanes.is_empty() {
        text.push_str("- No model lane receipts were produced.\n");
    } else {
        text.push_str(
            "| Lane | Provider | Model | Endpoint | Status | Fallback | HTTP | Reason |\n",
        );
        text.push_str("|---|---|---|---|---|---|---|---|\n");
        for receipt in &review.model_lanes {
            text.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` | `{}` | {} | {} | {} |\n",
                receipt.lane,
                receipt.provider,
                receipt.model,
                receipt.endpoint_kind,
                receipt.status,
                optional_str_cell(receipt.fallback_from.as_deref()),
                optional_u16_cell(receipt.http_status),
                escape_md(&receipt.reason)
            ));
        }
    }

    text.push_str("\n## Missing or failed model evidence\n\n");
    if review.model_mode == "off" {
        text.push_str("- Model evidence was intentionally disabled.\n");
    } else if missing_or_failed.is_empty() {
        text.push_str("- No planned model evidence is currently missing or failed.\n");
    } else {
        for item in missing_or_failed {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }
}

// Reporting only: these diagnostics do not change evidence admission or gate authority.
fn reporting_effective_model_lane_ids(out: &Path) -> Option<Vec<String>> {
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-plan.json")).ok()?).ok()?;
    value
        .pointer("/selectors/effective_model_lanes")?
        .as_array()?
        .iter()
        .map(|lane| {
            lane.as_str()
                .filter(|id| !id.trim().is_empty())
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn model_status_evidence_issues(
    review: &ReviewSummaryReceipt,
    selected: Option<&[String]>,
) -> Vec<String> {
    let mut issues = Vec::new();
    if review.model_mode != "auto" && review.model_mode != "off" {
        issues.push(
            "Model mode is unavailable or unsupported; model review is not established.".to_owned(),
        );
    }
    if let Some(selected) = selected.filter(|lanes| !lanes.is_empty()) {
        for lane in selected {
            if !review
                .model_lanes
                .iter()
                .any(|receipt| &receipt.lane == lane)
            {
                issues.push(format!(
                    "Lane `{lane}`: `receipt-absent`; selected model review output is unavailable."
                ));
            }
        }
    } else {
        issues.push("Effective model lane selection is unavailable or empty (`resolved-plan.json`); model review coverage is unproven.".to_owned());
    }
    for receipt in &review.provider_preflights {
        if receipt.status != "ok" {
            issues.push(format!(
                "Provider preflight `{}` model `{}` endpoint `{}`: `{}` - {}",
                receipt.provider,
                receipt.model,
                receipt.endpoint_kind,
                receipt.status,
                receipt.reason
            ));
        }
    }
    for receipt in &review.model_lanes {
        if receipt.status != "ok" {
            issues.push(format!(
                "Lane `{}` via `{}` model `{}` endpoint `{}`: `{}` - {}",
                receipt.lane,
                receipt.provider,
                receipt.model,
                receipt.endpoint_kind,
                receipt.status,
                receipt.reason
            ));
        }
    }
    issues
}

pub(crate) fn read_review_summary_receipt(out: &Path) -> Option<ReviewSummaryReceipt> {
    let text = fs::read_to_string(out.join("review/review.json")).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn optional_u16_cell(value: Option<u16>) -> String {
    value
        .map(|value| format!("`{value}`"))
        .unwrap_or_else(|| "-".to_owned())
}

pub(crate) fn optional_str_cell(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("`{}`", escape_md(value)))
        .unwrap_or_else(|| "-".to_owned())
}

pub(crate) fn evidence_label(sensor_id: &str) -> &'static str {
    match sensor_id {
        "tokmd" => "deterministic repository/diff packet",
        "cargo-allow" => "source-tree exception ledger",
        "ripr" => "Rust test-oracle packet",
        "unsafe-review" => "unsafe/native reviewability packet",
        "ast-grep" => "structural route scan",
        "semgrep" => "semantic security scan",
        "actionlint" => "workflow lint packet",
        "zizmor" => "workflow hardening packet",
        "gitleaks" => "secret-scan packet",
        "osv-scanner" => "dependency advisory packet",
        "cargo-audit" => "Cargo advisory packet",
        "cargo-deny" => "Cargo policy packet",
        _ => "sensor packet",
    }
}

pub(crate) fn read_sensor_receipt(path: &Path) -> Option<SensorReceipt> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn append_github_step_summary(summary: &str) -> Result<()> {
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") else {
        return Ok(());
    };
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    use std::io::Write as _;
    writeln!(file, "\n{summary}")?;
    Ok(())
}

pub(crate) fn escape_md(value: &str) -> String {
    value.replace('|', "\\|")
}

pub(crate) fn has_standalone_approval_line(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line
            .trim()
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim()
            .to_ascii_lowercase();
        matches!(
            trimmed.as_str(),
            "lgtm"
                | "looks good"
                | "clean"
                | "solid"
                | "no issues found"
                | "no actionable findings"
                | "no actionable"
                | "all checks passed"
        )
    })
}

pub(crate) const CORE_REVIEW_TOOLS: [&str; 6] = [
    "tokmd",
    "cargo-allow",
    "ripr",
    "unsafe-review",
    "ast-grep",
    "actionlint",
];
pub(crate) const STANDARD_IMAGE_TOKMD_VERSION: &str = "1.12.0";
pub(crate) const STANDARD_IMAGE_CARGO_ALLOW_VERSION: &str = "0.1.8";
// Core Rust sensors must not float on crates.io latest: the install script took
// crates.io latest, so image and local drifted apart silently (#316 — the
// dogfooded local ripr 0.5.0 lacked the subcommand the image's newer ripr
// accepted). Pins move together with scripts/install-gh-runner-tools.sh and
// scripts/install-review-image-tools.sh.
pub(crate) const STANDARD_IMAGE_RIPR_VERSION: &str = "0.10.0";
pub(crate) const STANDARD_IMAGE_UNSAFE_REVIEW_VERSION: &str = "0.3.4";
// The actionlint version deliberately omits the `v` prefix: the doctor
// version-matcher parses `actionlint version v1.7.12` and strips the `v`
// before comparing to this constant. The install scripts and docs use the
// `v`-prefixed form (`v1.7.12`) for `go install ...@v1.7.12`. Both spellings
// are correct for their context; this is not drift (#610).
pub(crate) const STANDARD_IMAGE_ACTIONLINT_VERSION: &str = "1.7.12";

pub(crate) fn is_core_review_tool(tool_id: &str) -> bool {
    CORE_REVIEW_TOOLS.contains(&tool_id)
}

pub(crate) fn expected_standard_image_tool_version(tool_id: &str) -> Option<&'static str> {
    match tool_id {
        "tokmd" => Some(STANDARD_IMAGE_TOKMD_VERSION),
        "cargo-allow" => Some(STANDARD_IMAGE_CARGO_ALLOW_VERSION),
        "ripr" => Some(STANDARD_IMAGE_RIPR_VERSION),
        "unsafe-review" => Some(STANDARD_IMAGE_UNSAFE_REVIEW_VERSION),
        "actionlint" => Some(STANDARD_IMAGE_ACTIONLINT_VERSION),
        _ => None,
    }
}

pub(crate) fn doctor_tool_install_hint(tool_id: &str) -> String {
    match tool_id {
        "tokmd" => format!(
            "cargo install tokmd --locked --version {} --force",
            STANDARD_IMAGE_TOKMD_VERSION
        ),
        "cargo-allow" => format!(
            "cargo install cargo-allow --locked --version {} --force",
            STANDARD_IMAGE_CARGO_ALLOW_VERSION
        ),
        "ripr" => format!(
            "cargo install ripr --locked --version {} --force",
            STANDARD_IMAGE_RIPR_VERSION
        ),
        "unsafe-review" => format!(
            "cargo install unsafe-review --locked --version {} --force",
            STANDARD_IMAGE_UNSAFE_REVIEW_VERSION
        ),
        "ast-grep" => "npm install -g @ast-grep/cli".to_owned(),
        "actionlint" => format!(
            "go install github.com/rhysd/actionlint/cmd/actionlint@v{}; add $(go env GOPATH)/bin to PATH",
            STANDARD_IMAGE_ACTIONLINT_VERSION
        ),
        _ => format!("install `{tool_id}` and make it available on PATH"),
    }
}

pub(crate) fn doctor_tool_version_fix(tool_id: &str, expected: &str) -> String {
    match tool_id {
        "tokmd" => format!("cargo install tokmd --locked --version {expected} --force"),
        "cargo-allow" => {
            format!("cargo install cargo-allow --locked --version {expected} --force")
        }
        "ripr" => format!("cargo install ripr --locked --version {expected} --force"),
        "unsafe-review" => {
            format!("cargo install unsafe-review --locked --version {expected} --force")
        }
        "actionlint" => {
            format!(
                "go install github.com/rhysd/actionlint/cmd/actionlint@v{expected}; add $(go env GOPATH)/bin to PATH"
            )
        }
        _ => doctor_tool_install_hint(tool_id),
    }
}

pub(crate) fn command_version_matches(actual: &str, expected: &str) -> bool {
    actual
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | '(' | ')'))
        .any(|part| part.trim_start_matches('v') == expected)
}

pub(crate) fn cache_root_path(value: Option<&PathBuf>) -> PathBuf {
    value
        .cloned()
        .or_else(|| std::env::var_os("UB_REVIEW_CACHE_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(".cache/ub-review"))
}

pub(crate) fn base_cache_dir(cache_root: &Path, base_tree_sha: &str) -> PathBuf {
    cache_root.join("bases").join(base_tree_sha)
}

pub(crate) fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane_receipt(lane: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "lane": lane,
            "provider": "minimax",
            "model": "test-model",
            "endpoint_kind": "openai-chat",
            "status": status,
            "reason": if status == "skipped" { "model-mode off" } else { status },
            "duration_ms": null,
            "http_status": null,
            "response_shape": null,
            "fallback_from": null
        })
    }

    fn reporting_summary(
        review: Option<&str>,
        selection: Option<serde_json::Value>,
    ) -> Result<String> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::create_dir_all(out.join("review"))?;
        if let Some(review) = review {
            fs::write(out.join("review/review.json"), review)?;
        }
        if let Some(selection) = selection {
            fs::write(
                out.join("resolved-plan.json"),
                serde_json::to_vec(&serde_json::json!({
                    "selectors": { "effective_model_lanes": selection }
                }))?,
            )?;
        }
        render_summary(
            out,
            &crate::tests::test_plan(Vec::new()),
            &crate::tests::test_diff(),
        )
    }

    #[test]
    fn reporting_model_review_requires_complete_selected_receipts() -> Result<()> {
        let cases = [
            ("empty", vec![], "incomplete", "Lane `ub`: `receipt-absent`"),
            (
                "planned",
                vec![
                    lane_receipt("ub", "planned"),
                    lane_receipt("tests", "planned"),
                ],
                "incomplete",
                "Lane `ub` via `minimax` model `test-model` endpoint `openai-chat`: `planned`",
            ),
            (
                "running",
                vec![
                    lane_receipt("ub", "running"),
                    lane_receipt("tests", "running"),
                ],
                "incomplete",
                "Lane `tests` via `minimax` model `test-model` endpoint `openai-chat`: `running`",
            ),
            (
                "partially absent",
                vec![lane_receipt("ub", "ok"), lane_receipt("ub", "ok")],
                "incomplete",
                "Lane `tests`: `receipt-absent`",
            ),
            (
                "rejected",
                vec![lane_receipt("ub", "rejected"), lane_receipt("tests", "ok")],
                "incomplete",
                "Lane `ub` via `minimax` model `test-model` endpoint `openai-chat`: `rejected`",
            ),
            (
                "missing key",
                vec![
                    lane_receipt("ub", "missing_key"),
                    lane_receipt("tests", "ok"),
                ],
                "incomplete",
                "Lane `ub` via `minimax` model `test-model` endpoint `openai-chat`: `missing_key`",
            ),
            (
                "invalid JSON",
                vec![
                    lane_receipt("ub", "invalid_json"),
                    lane_receipt("tests", "ok"),
                ],
                "incomplete",
                "Lane `ub` via `minimax` model `test-model` endpoint `openai-chat`: `invalid_json`",
            ),
            (
                "completed",
                vec![lane_receipt("ub", "ok"), lane_receipt("tests", "ok")],
                "completed",
                "every selected lane has a usable model receipt",
            ),
            (
                "degraded",
                vec![lane_receipt("ub", "degraded"), lane_receipt("tests", "ok")],
                "limited",
                "selected lanes have usable model receipts with reported limitations",
            ),
        ];
        for (name, lanes, status, detail) in cases {
            let review = serde_json::json!({"model_mode": "auto", "model_lanes": lanes});
            let summary = reporting_summary(
                Some(&serde_json::to_string(&review)?),
                Some(serde_json::json!(["ub", "tests"])),
            )?;
            assert!(
                summary.contains(&format!("- Model review: `{status}`")),
                "{name}: {summary}"
            );
            assert!(summary.contains(detail), "{name}: {summary}");
            assert_eq!(
                summary.contains("No planned model evidence is currently missing or failed."),
                status == "completed",
                "{name}: {summary}"
            );
        }
        Ok(())
    }

    #[test]
    fn reporting_model_review_preserves_unavailable_receipts_and_selection() -> Result<()> {
        for review in [None, Some("{"), Some("{\"model_lanes\":false}")] {
            let summary = reporting_summary(review, Some(serde_json::json!(["ub", "tests"])))?;
            assert!(summary.contains("- Model review: `incomplete`; `review/review.json` is missing, unreadable, or invalid."), "{summary}");
            assert!(!summary.contains("No planned model evidence is currently missing or failed."));
        }
        let review = serde_json::to_string(&serde_json::json!({
            "model_mode": "auto", "model_lanes": [lane_receipt("ub", "ok")]
        }))?;
        for selection in [
            None,
            Some(serde_json::json!(["ub", 7])),
            Some(serde_json::json!([])),
        ] {
            let summary = reporting_summary(Some(&review), selection)?;
            assert!(
                summary.contains("- Model review: `incomplete`"),
                "{summary}"
            );
            assert!(
                summary.contains("Effective model lane selection is unavailable or empty"),
                "{summary}"
            );
            assert!(!summary.contains("No planned model evidence is currently missing or failed."));
        }
        // Profile lanes that were not selected must not become missing obligations.
        let summary = reporting_summary(Some(&review), Some(serde_json::json!(["ub"])))?;
        assert!(summary.contains("- Model review: `completed`"), "{summary}");
        assert!(!summary.contains("Lane `tests`: `receipt-absent`"));
        Ok(())
    }

    #[test]
    fn reporting_model_off_is_intentional_and_never_model_review_success() -> Result<()> {
        for lanes in [vec![], vec![lane_receipt("ub", "skipped")]] {
            let review = serde_json::to_string(&serde_json::json!({
                "model_mode": "off", "model_lanes": lanes
            }))?;
            let summary = reporting_summary(Some(&review), None)?;
            assert!(summary.contains("- Model review: `not_attempted`; model mode off supports setup, testing, or fallback; no model review was requested."), "{summary}");
            assert!(!summary.contains("- Model review: `completed`"));
            assert!(!summary.contains("No planned model evidence is currently missing or failed."));
            assert!(summary.contains("Model evidence was intentionally disabled."));
        }
        Ok(())
    }
}

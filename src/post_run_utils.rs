//! Post-run utilities: model status sections, GitHub step summary,
//! doctor tool helpers, cache/env utilities, and profile config hash
//! (cleanup train step 54, pure code motion).

use crate::*;

pub(crate) fn render_model_status_sections(text: &mut String, out: &Path) {
    let Some(review) = read_review_summary_receipt(out) else {
        return;
    };

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

    let missing_or_failed = model_status_evidence_issues(&review);
    text.push_str("\n## Missing or failed model evidence\n\n");
    if missing_or_failed.is_empty() {
        text.push_str("- No planned model evidence is currently missing or failed.\n");
    } else {
        for item in missing_or_failed {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }
}

pub(crate) fn model_status_evidence_issues(review: &ReviewSummaryReceipt) -> Vec<String> {
    let mut issues = Vec::new();
    for receipt in &review.provider_preflights {
        if is_model_evidence_issue(&receipt.status) {
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
        if is_model_receipt_evidence_issue(receipt) {
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
pub(crate) const STANDARD_IMAGE_RIPR_VERSION: &str = "0.8.0";
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

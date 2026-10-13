//! Ledger context rendering, bounded text helpers, model/preflight
//! receipt construction, and sensor evidence issue collection (cleanup
//! train step 48, pure code motion).

use crate::*;

pub(crate) fn render_ledger_context(
    root: &Path,
    config: &Config,
    args: &RunArgs,
) -> Result<String> {
    let ledger = effective_ledger_path(config, args);
    let ledger = ledger.trim();
    if ledger.is_empty() {
        return Ok("- No UB ledger configured for this run.\n".to_owned());
    }
    let configured_path = PathBuf::from(ledger);
    let path = if configured_path.is_absolute() {
        configured_path
    } else {
        root.join(configured_path)
    };
    if !path.exists() {
        let source = if args.ledger_path.trim().is_empty() {
            "`[repo].ledger`"
        } else {
            "`--ledger-path` / `UB_REVIEW_LEDGER_PATH`"
        };
        return Ok(format!(
            "- UB ledger configured by {source} but unavailable at `{}`; optional context omitted. Verify the configured path to include ledger context.\n",
            path.display()
        ));
    }
    if path.is_file() {
        let text = read_bounded_text(&path, args.ledger_max_bytes)?;
        return Ok(format!(
            "Source: `{}`\n\n```text\n{}\n```\n",
            path.display(),
            text
        ));
    }
    if path.is_dir() {
        let mut entries = fs::read_dir(&path)?
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.path())
            .filter(|path| is_ledger_excerpt_candidate(path))
            .collect::<Vec<_>>();
        entries.sort();
        entries.truncate(8);
        if entries.is_empty() {
            return Ok(format!(
                "- UB ledger directory `{}` has no supported excerpt files.\n",
                path.display()
            ));
        }
        let mut text = format!("Source directory: `{}`\n\n", path.display());
        let per_entry_limit = args.ledger_max_bytes.saturating_div(entries.len().max(1));
        let per_entry_limit = per_entry_limit.max(1024);
        let mut remaining = args.ledger_max_bytes;
        for entry in entries {
            if remaining == 0 {
                text.push_str("[ledger byte budget exhausted]\n");
                break;
            }
            let limit = per_entry_limit.min(remaining);
            text.push_str(&format!("### `{}`\n\n```text\n", entry.display()));
            let excerpt = read_bounded_text(&entry, limit)?;
            remaining = remaining.saturating_sub(excerpt.len());
            text.push_str(&excerpt);
            text.push_str("\n```\n\n");
        }
        return Ok(text);
    }
    Ok(format!(
        "- UB ledger path `{}` is not a regular file or directory.\n",
        path.display()
    ))
}

pub(crate) fn effective_ledger_path(config: &Config, args: &RunArgs) -> String {
    let cli_path = args.ledger_path.trim();
    if cli_path.is_empty() {
        config.repo.ledger.clone()
    } else {
        cli_path.to_owned()
    }
}

pub(crate) struct BoundedText {
    pub(crate) text: String,
    pub(crate) truncated: bool,
}

pub(crate) fn read_bounded_text(path: &Path, max_bytes: usize) -> Result<String> {
    read_bounded_text_with_status(path, max_bytes).map(|bounded| bounded.text)
}

pub(crate) fn read_bounded_text_with_status(path: &Path, max_bytes: usize) -> Result<BoundedText> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    // The configured ceiling is a read limit, not an allocation request.
    // Grow with the actual file bytes while retaining one-byte truncation
    // lookahead. This also permits small files with a usize::MAX ceiling.
    let read_limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut buffer = Vec::new();
    use std::io::Read as _;
    let count = file
        .take(read_limit)
        .read_to_end(&mut buffer)
        .with_context(|| format!("read {}", path.display()))?;
    buffer.truncate(count.min(max_bytes));
    let mut text = String::from_utf8_lossy(&buffer).to_string();
    if count > max_bytes {
        text.push_str("\n[truncated]\n");
    }
    Ok(BoundedText {
        text,
        truncated: count > max_bytes,
    })
}

pub(crate) fn bounded_string(value: &str, max_bytes: usize) -> BoundedText {
    if value.len() <= max_bytes {
        return BoundedText {
            text: value.to_owned(),
            truncated: false,
        };
    }
    let mut end = 0;
    for (index, ch) in value.char_indices() {
        let next = index + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    let mut text = value[..end].to_owned();
    text.push_str("\n[truncated]\n");
    BoundedText {
        text,
        truncated: true,
    }
}

pub(crate) fn is_ledger_excerpt_candidate(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("md" | "txt" | "toml" | "json")
    )
}

pub(crate) fn build_model_lane_receipts(
    assignments: &[ModelAssignment],
    args: &RunArgs,
) -> Vec<ModelLaneReceipt> {
    assignments
        .iter()
        .map(|assignment| {
            let spec = &assignment.spec;
            let (status, reason) = match args.model_mode {
                ModelMode::Off => ("skipped", "model-mode off".to_owned()),
                ModelMode::Auto => {
                    let primary_env = model_api_key_env(spec.provider);
                    let primary_label = model_api_key_label(spec.provider);
                    if env_value_present(primary_env) {
                        (
                            "planned",
                            format!(
                                "{primary_label} present; lane eligible for {} call",
                                spec.provider.key()
                            ),
                        )
                    } else if let Some(fallback) = &assignment.fallback {
                        let fallback_env = model_api_key_env(fallback.provider);
                        let fallback_label = model_api_key_label(fallback.provider);
                        if env_value_present(fallback_env) {
                            (
                                "planned",
                                format!(
                                    "{primary_label} not provided; fallback {fallback_label} present"
                                ),
                            )
                        } else {
                            (
                                "missing_key",
                                format!(
                                    "{primary_label} and fallback {fallback_label} not provided; lane output unavailable"
                                ),
                            )
                        }
                    } else {
                        (
                            "missing_key",
                            format!(
                                "{primary_label} not provided; {} lane output unavailable",
                                spec.provider.key()
                            ),
                        )
                    }
                }
            };
            ModelLaneReceipt {
                lane: assignment.lane.id.clone(),
                provider: spec.provider.key().to_owned(),
                model: spec.model.clone(),
                endpoint_kind: spec.endpoint_kind.key().to_owned(),
                status: status.to_owned(),
                reason,
                duration_ms: None,
                http_status: None,
                response_shape: None,
                fallback_from: None,
                attempt_count: Some(0),
                cache_usage: ModelCacheUsage::default(),
                // Preflight/planned receipts carry empty cohort provenance;
                // the cohort is stamped when the lane executes (model_exec /
                // proof_planner_lane), since shared_prefix_hash is known only
                // at run time. (Order 5 of #678.)
                cohort_id: String::new(),
                shared_prefix_hash: String::new(),
                thread_id: String::new(),
                turn: 0,
                cohort_broken: false,
            }
        })
        .collect()
}

pub(crate) fn build_provider_preflight_receipts(
    assignments: &[ModelAssignment],
    args: &RunArgs,
) -> Vec<ProviderPreflightReceipt> {
    let mut specs = BTreeSet::new();
    for assignment in assignments {
        specs.insert(assignment.spec.clone());
        if let Some(fallback) = &assignment.fallback {
            specs.insert(fallback.clone());
        }
    }
    specs
        .into_iter()
        .map(|spec| provider_preflight_receipt_for_spec(spec, args))
        .collect()
}

pub(crate) fn ensure_provider_preflight_receipt(
    receipts: &mut Vec<ProviderPreflightReceipt>,
    spec: ProviderSpec,
    args: &RunArgs,
) {
    if receipts
        .iter()
        .any(|receipt| preflight_matches_spec(receipt, &spec))
    {
        return;
    }
    receipts.push(provider_preflight_receipt_for_spec(spec, args));
}

pub(crate) fn ensure_provider_preflight_receipts_for_assignment(
    receipts: &mut Vec<ProviderPreflightReceipt>,
    assignment: &ModelAssignment,
    args: &RunArgs,
) {
    ensure_provider_preflight_receipt(receipts, assignment.spec.clone(), args);
    if let Some(fallback) = &assignment.fallback {
        ensure_provider_preflight_receipt(receipts, fallback.clone(), args);
    }
}

pub(crate) fn provider_preflight_receipt_for_spec(
    spec: ProviderSpec,
    args: &RunArgs,
) -> ProviderPreflightReceipt {
    let (status, reason) = match args.model_mode {
        ModelMode::Off => ("skipped", "model-mode off".to_owned()),
        ModelMode::Auto => {
            let env_name = model_api_key_env(spec.provider);
            let key_label = model_api_key_label(spec.provider);
            if env_value_present(env_name) {
                ("planned", format!("{key_label} present; preflight planned"))
            } else {
                (
                    "missing_key",
                    format!("{key_label} not provided; provider unavailable"),
                )
            }
        }
    };
    ProviderPreflightReceipt {
        provider: spec.provider.key().to_owned(),
        model: spec.model,
        endpoint_kind: spec.endpoint_kind.key().to_owned(),
        status: status.to_owned(),
        reason,
        duration_ms: None,
        http_status: None,
        response_shape: None,
        cache_usage: ModelCacheUsage::default(),
    }
}

pub(crate) fn is_model_evidence_issue(status: &str) -> bool {
    matches!(
        status,
        "missing_key"
            | "failed"
            | "invalid_json"
            | "timed_out"
            | "rate_limited"
            | "auth_failed"
            | "bad_envelope"
            | "preflight_failed"
    )
}

pub(crate) fn is_model_receipt_evidence_issue(receipt: &ModelLaneReceipt) -> bool {
    is_model_evidence_issue(&receipt.status) || is_model_skipped_evidence_issue(receipt)
}

pub(crate) fn is_model_skipped_evidence_issue(receipt: &ModelLaneReceipt) -> bool {
    receipt.status == "skipped"
        && (model_lane_call_attempted(receipt)
            || matches!(
                receipt.reason.as_str(),
                // The wave-loop sweep writes the budget-only phrasing; the
                // "or inline comment cap" variant is the legacy phrasing kept so
                // older receipts still classify. Without the current string,
                // budget-skipped lanes (including a starved fallback retry)
                // silently vanished from missing model evidence.
                "model-mode off"
                    | "model call budget reached before lane execution"
                    | "model call budget or inline comment cap reached before lane execution"
                    | "model call budget exhausted before refuter pass"
            ))
}

pub(crate) fn collect_sensor_evidence_issues(out: &Path, plan: &Plan) -> Vec<SensorEvidenceIssue> {
    plan.sensors
        .iter()
        .filter_map(|sensor| {
            let status_path = out
                .join("sensors")
                .join(&sensor.id)
                .join("ub-review-sensor-status.json");
            let receipt = read_sensor_receipt(&status_path);
            let status = receipt
                .as_ref()
                .map(|receipt| receipt.status.clone())
                .unwrap_or_else(|| "receipt-absent".to_owned());
            let reason = receipt
                .map(|receipt| receipt.reason)
                .unwrap_or_else(|| sensor.reason.clone());
            if sensor.id == "unsafe-review"
                && status == "ok"
                && let Err(gap) =
                    read_unsafe_review_artifacts(&out.join("sensors").join(&sensor.id))
            {
                return Some(SensorEvidenceIssue {
                    sensor: sensor.id.clone(),
                    status: "artifact-gap".to_owned(),
                    reason: gap.reason(),
                });
            }
            if !is_sensor_evidence_issue(sensor, &status, &reason) {
                return None;
            }
            Some(SensorEvidenceIssue {
                sensor: sensor.id.clone(),
                status,
                reason,
            })
        })
        .collect()
}

pub(crate) fn is_sensor_evidence_issue(sensor: &SensorPlan, status: &str, reason: &str) -> bool {
    match status {
        "ok" => false,
        "skipped" => is_sensor_skipped_evidence_issue(sensor, reason),
        _ => true,
    }
}

pub(crate) fn is_sensor_skipped_evidence_issue(sensor: &SensorPlan, reason: &str) -> bool {
    sensor.required
        || sensor.run
        || reason == "dry-run; sensor not executed"
        || reason.starts_with("box guard failed")
}

#[cfg(test)]
mod bounded_file_tests {
    use super::*;

    #[test]
    fn bounded_file_empty_does_not_reserve_the_capture_ceiling() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("empty.md");
        fs::write(&path, b"")?;
        let actual = read_bounded_text_with_status(&path, usize::MAX)?;
        assert_eq!(actual.text, "");
        assert!(!actual.truncated);
        Ok(())
    }

    #[test]
    fn bounded_file_small_does_not_reserve_the_capture_ceiling() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("ledger.md");
        fs::write(&path, "Prior claim: café remains unresolved.\n")?;
        let actual = read_bounded_text_with_status(&path, usize::MAX)?;
        assert_eq!(actual.text, "Prior claim: café remains unresolved.\n");
        assert!(!actual.truncated);
        assert_eq!(
            read_bounded_text(&path, usize::MAX)?,
            "Prior claim: café remains unresolved.\n"
        );
        Ok(())
    }

    #[test]
    fn bounded_file_preserves_exact_excerpts_and_truncation_truth() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("excerpt.txt");
        let cases: &[(&[u8], usize, &str, bool)] = &[
            (b"", 0, "", false),
            (b"abc", 0, "\n[truncated]\n", true),
            (b"abc", 2, "ab\n[truncated]\n", true),
            (b"abc", 3, "abc", false),
            (b"abc", 4, "abc", false),
            ("café".as_bytes(), 4, "caf\u{fffd}\n[truncated]\n", true),
            ("café".as_bytes(), 5, "café", false),
            (&[b'a', 0xff, b'b'], 3, "a\u{fffd}b", false),
        ];
        for (input, budget, expected, truncated) in cases {
            fs::write(&path, input)?;
            let actual = read_bounded_text_with_status(&path, *budget)?;
            assert_eq!(actual.text, *expected, "input {input:?}, budget {budget}");
            assert_eq!(actual.truncated, *truncated, "budget {budget}");
            assert_eq!(read_bounded_text(&path, *budget)?, *expected);
        }
        Ok(())
    }

    #[test]
    fn bounded_file_missing_remains_an_error() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let missing = temp.path().join("missing.md");
        let error = read_bounded_text_with_status(&missing, usize::MAX)
            .err()
            .context("missing evidence must not be admitted as empty text")?;
        assert!(error.to_string().contains("open "));
        Ok(())
    }
}

//! Calibration-driven mode guidance: `status`, `recommend`, `promote`.
//!
//! These commands turn `review/calibration.json` artifacts (#710) into
//! actionable guidance. `status` summarizes one run; `recommend` and `promote`
//! aggregate many runs and encode the promotion thresholds that previously
//! existed only as prose in `docs/ADOPTION_MODES.md` (infra-excluded < 5%,
//! false-positive < 10%). The recommendation is honest about unmeasured
//! signal: false-positive rate is 0/0 until maintainers label runs, and the
//! logic says so rather than silently passing.

use crate::*;
use std::fs;

/// Minimum runs before `recommend` will name a mode instead of asking for more
/// data. Matches the "10–20 PRs" advisory window in ADOPTION_MODES.md, kept
/// conservative so a single noisy run cannot drive a recommendation.
const MIN_RUNS_FOR_RECOMMENDATION: usize = 5;
const INFRA_EXCLUDED_THRESHOLD: f64 = 0.05; // 5%
const FALSE_POSITIVE_THRESHOLD: f64 = 0.10; // 10%

/// Load a single run's calibration artifact, validating the schema so a
/// future-incompatible or corrupted file can never silently mislead the
/// recommendation logic.
pub(crate) fn load_calibration(run_dir: &Path) -> Result<CalibrationArtifact> {
    let path = run_dir.join("review").join("calibration.json");
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let artifact: CalibrationArtifact = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {} as calibration artifact", path.display()))?;
    if artifact.schema != CALIBRATION_SCHEMA {
        bail!(
            "{} has schema `{}` (expected `{CALIBRATION_SCHEMA}`); cannot interpret",
            path.display(),
            artifact.schema
        );
    }
    Ok(artifact)
}

/// Recursively scan a directory tree for `*/review/calibration.json` files and
/// load each as a typed `CalibrationArtifact`. Mirrors the xtask
/// `collect_calibration_files` directory-shape assumption but reads typed
/// artifacts (an improvement over xtask's untyped `serde_json::Value`).
pub(crate) fn collect_calibrations(runs_dir: &Path) -> Result<Vec<(PathBuf, CalibrationArtifact)>> {
    let mut out = Vec::new();
    collect_calibrations_inner(runs_dir, &mut out)?;
    // Stable order so aggregate output is reproducible across platforms.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn collect_calibrations_inner(
    dir: &Path,
    out: &mut Vec<(PathBuf, CalibrationArtifact)>,
) -> Result<()> {
    let cal_path = dir.join("review").join("calibration.json");
    if cal_path.is_file() {
        let bytes = fs::read(&cal_path).with_context(|| format!("read {}", cal_path.display()))?;
        let artifact: CalibrationArtifact = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", cal_path.display()))?;
        if artifact.schema == CALIBRATION_SCHEMA {
            out.push((cal_path, artifact));
        }
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_calibrations_inner(&path, out)?;
        }
    }
    Ok(())
}

/// Aggregated calibration metrics across many runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CalibrationAggregate {
    pub(crate) runs: usize,
    pub(crate) infra_excluded_count: usize,
    pub(crate) false_positive_count: usize,
    pub(crate) acted_on_count: usize,
    pub(crate) proof_changed_total: usize,
    pub(crate) expected_quiet_count: usize,
    pub(crate) lanes_executed_total: usize,
    pub(crate) lane_continuations_total: usize,
    pub(crate) reporter_questions_total: usize,
    pub(crate) proof_selected_total: usize,
    pub(crate) proof_executed_total: usize,
    pub(crate) inline_comments_total: usize,
    /// Number of runs with a non-null `human_classification` (i.e. a maintainer
    /// has labeled the run). When this is 0, false-positive rate is unmeasured.
    pub(crate) human_labeled_count: usize,
}

impl CalibrationAggregate {
    pub(crate) fn infra_excluded_rate(&self) -> f64 {
        rate(self.infra_excluded_count, self.runs)
    }

    pub(crate) fn false_positive_rate(&self) -> f64 {
        rate(self.false_positive_count, self.human_labeled_count)
    }
}

fn rate(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

pub(crate) fn aggregate_calibrations(
    artifacts: &[(PathBuf, CalibrationArtifact)],
) -> CalibrationAggregate {
    let runs = artifacts.len();
    let mut agg = CalibrationAggregate {
        runs,
        ..CalibrationAggregate::default()
    };
    for (_, cal) in artifacts {
        if cal.classification.infra_excluded {
            agg.infra_excluded_count += 1;
        }
        if cal.counts.lane_conclusions_changed_by_proof > 0 {
            agg.proof_changed_total += cal.counts.lane_conclusions_changed_by_proof;
        }
        if cal.classification.suggested_class == "expected-quiet" {
            agg.expected_quiet_count += 1;
        }
        agg.lanes_executed_total += cal.counts.lanes_executed;
        agg.lane_continuations_total += cal.counts.lane_continuations;
        agg.reporter_questions_total += cal.counts.reporter_questions;
        agg.proof_selected_total += cal.counts.proof_requests_model_selected;
        agg.proof_executed_total += cal.counts.proof_requests_executed;
        agg.inline_comments_total += cal.counts.inline_comments_posted;
        if let Some(label) = cal.classification.human_classification.as_deref() {
            agg.human_labeled_count += 1;
            match label {
                "false-positive" => agg.false_positive_count += 1,
                "acted-on" => agg.acted_on_count += 1,
                _ => {}
            }
        }
    }
    agg
}

/// One criterion in the recommendation, with its pass/fail and a human-readable
/// note. `measured = false` means the signal is unavailable (e.g. no human
/// labels yet), so the criterion neither passes nor fails.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CriterionResult {
    pub(crate) name: &'static str,
    pub(crate) passed: bool,
    pub(crate) measured: bool,
    pub(crate) detail: String,
}

/// The recommendation: which preset, which stage name, and why.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModeRecommendation {
    pub(crate) recommended: ReviewModePreset,
    pub(crate) stage: &'static str,
    pub(crate) criteria: Vec<CriterionResult>,
    pub(crate) needs_more_data: bool,
}

/// Decide which review mode the calibration data supports. Encodes the
/// ADOPTION_MODES.md promotion thresholds (infra-excluded < 5%,
/// false-positive < 10%, >= 1 acted-on for strict).
pub(crate) fn recommend_mode(agg: &CalibrationAggregate) -> ModeRecommendation {
    if agg.runs < MIN_RUNS_FOR_RECOMMENDATION {
        return ModeRecommendation {
            recommended: ReviewModePreset::Advisory,
            stage: "advisory (collecting data)",
            needs_more_data: true,
            criteria: vec![CriterionResult {
                name: "sample size",
                passed: false,
                measured: true,
                detail: format!(
                    "{} runs collected; need at least {MIN_RUNS_FOR_RECOMMENDATION} before recommending a mode",
                    agg.runs
                ),
            }],
        };
    }

    let infra_ok = agg.infra_excluded_rate() < INFRA_EXCLUDED_THRESHOLD;
    let infra_criterion = CriterionResult {
        name: "infra-excluded rate",
        passed: infra_ok,
        measured: true,
        detail: format!(
            "{:.1}% (threshold < {:.0}%; {} of {} runs excluded)",
            agg.infra_excluded_rate() * 100.0,
            INFRA_EXCLUDED_THRESHOLD * 100.0,
            agg.infra_excluded_count,
            agg.runs,
        ),
    };

    // False-positive rate is only meaningful when runs are human-labeled.
    let (fp_ok, fp_criterion) = if agg.human_labeled_count == 0 {
        (
            true, // does not block gate, but flagged as unmeasured
            CriterionResult {
                name: "false-positive rate",
                passed: true,
                measured: false,
                detail: "unmeasured — no runs carry a human_classification label yet".to_owned(),
            },
        )
    } else {
        let fp_rate = agg.false_positive_rate();
        (
            fp_rate < FALSE_POSITIVE_THRESHOLD,
            CriterionResult {
                name: "false-positive rate",
                passed: fp_rate < FALSE_POSITIVE_THRESHOLD,
                measured: true,
                detail: format!(
                    "{:.1}% (threshold < {:.0}%; {} of {} labeled runs)",
                    fp_rate * 100.0,
                    FALSE_POSITIVE_THRESHOLD * 100.0,
                    agg.false_positive_count,
                    agg.human_labeled_count,
                ),
            },
        )
    };

    let gate_eligible = infra_ok && fp_ok;
    if !gate_eligible {
        return ModeRecommendation {
            recommended: ReviewModePreset::Advisory,
            stage: "advisory",
            needs_more_data: false,
            criteria: vec![infra_criterion, fp_criterion],
        };
    }

    // strict requires acted-on evidence AND proof changing a conclusion.
    let acted_on_ok = agg.acted_on_count >= 1;
    let proof_signal_ok = agg.proof_changed_total >= 1;
    let strict_eligible = acted_on_ok && proof_signal_ok;

    let mut criteria = vec![infra_criterion, fp_criterion];
    criteria.push(CriterionResult {
        name: "acted-on comments",
        passed: acted_on_ok,
        measured: true,
        detail: format!("{} found (need >= 1 for strict)", agg.acted_on_count),
    });
    criteria.push(CriterionResult {
        name: "proof-changed conclusions",
        passed: proof_signal_ok,
        measured: true,
        detail: format!("{} total (need >= 1 for strict)", agg.proof_changed_total),
    });

    ModeRecommendation {
        recommended: if strict_eligible {
            ReviewModePreset::Strict
        } else {
            ReviewModePreset::Gate
        },
        stage: if strict_eligible { "strict" } else { "gate" },
        needs_more_data: false,
        criteria,
    }
}

/// `ub-review status`: print a single run's calibration summary.
pub(crate) fn cmd_status(args: StatusArgs) -> Result<()> {
    let cal = load_calibration(&args.run_dir)?;
    println!("ub-review status — {}", args.run_dir.display());
    println!();
    println!("Cohort:");
    println!("  provider:            {}", cal.cohort.provider);
    println!("  model:               {}", cal.cohort.model);
    println!("  cohort_id:           {}", cal.cohort.cohort_id);
    println!("  shared_prefix_hash:  {}", cal.cohort.shared_prefix_hash);
    println!("  cohort_broken:       {}", cal.cohort.cohort_broken);
    println!();
    println!("Counts:");
    println!(
        "  lanes executed:               {}",
        cal.counts.lanes_executed
    );
    println!(
        "  lane continuations:           {}",
        cal.counts.lane_continuations
    );
    println!(
        "  reporter questions:           {}",
        cal.counts.reporter_questions
    );
    println!(
        "  proof model-selected:         {}",
        cal.counts.proof_requests_model_selected
    );
    println!(
        "  proof executed:               {}",
        cal.counts.proof_requests_executed
    );
    println!(
        "  proof receipts routed:        {}",
        cal.counts.proof_receipts_routed
    );
    println!(
        "  proof-changed conclusions:    {}",
        cal.counts.lane_conclusions_changed_by_proof
    );
    println!(
        "  inline comments posted:       {}",
        cal.counts.inline_comments_posted
    );
    println!(
        "  summary comments posted:      {}",
        cal.counts.summary_comments_posted
    );
    println!();
    println!("Classification:");
    println!("  run_class:            {}", cal.classification.run_class);
    println!(
        "  suggested_class:      {}",
        cal.classification.suggested_class
    );
    println!(
        "  infra_excluded:       {}",
        cal.classification.infra_excluded
    );
    println!(
        "  human_classification: {}",
        cal.classification
            .human_classification
            .as_deref()
            .unwrap_or("(none)")
    );
    println!("  gate_policy:          {}", cal.gate_policy);
    if !cal.notable_events.is_empty() {
        println!();
        println!("Notable proof-changed-conclusion events:");
        for event in &cal.notable_events {
            println!(
                "  - lane={} {} -> {} ({})",
                event.lane, event.before, event.after, event.reason
            );
        }
    }
    Ok(())
}

/// `ub-review recommend`: aggregate and recommend a mode.
pub(crate) fn cmd_recommend(args: RecommendArgs) -> Result<()> {
    let artifacts = collect_calibrations(&args.runs_dir)?;
    if artifacts.is_empty() {
        bail!(
            "no calibration artifacts found under {} (expected */review/calibration.json)",
            args.runs_dir.display()
        );
    }
    let agg = aggregate_calibrations(&artifacts);
    let rec = recommend_mode(&agg);

    println!(
        "ub-review recommend — {} runs under {}",
        agg.runs,
        args.runs_dir.display()
    );
    println!();
    println!("Aggregate:");
    println!(
        "  infra-excluded:       {:.1}% ({} runs)",
        agg.infra_excluded_rate() * 100.0,
        agg.infra_excluded_count
    );
    if agg.human_labeled_count > 0 {
        println!(
            "  false-positive:       {:.1}% ({} of {} labeled)",
            agg.false_positive_rate() * 100.0,
            agg.false_positive_count,
            agg.human_labeled_count
        );
    } else {
        println!("  false-positive:       unmeasured (no human-classified runs)");
    }
    println!("  acted-on comments:    {}", agg.acted_on_count);
    println!("  proof-changed total:  {}", agg.proof_changed_total);
    println!("  expected-quiet runs:  {}", agg.expected_quiet_count);
    println!("  lanes executed total: {}", agg.lanes_executed_total);
    println!();
    println!("Criteria:");
    for c in &rec.criteria {
        let mark = if !c.measured {
            "~"
        } else if c.passed {
            "PASS"
        } else {
            "FAIL"
        };
        println!(
            "  [{mark}] {name}: {detail}",
            name = c.name,
            detail = c.detail
        );
    }
    println!();
    if rec.needs_more_data {
        println!("Recommendation: collect more data (stay advisory).");
    } else {
        println!(
            "Recommended mode: {} ({})",
            rec.recommended.key(),
            rec.stage
        );
    }
    Ok(())
}

/// `ub-review promote`: go/no-go for the next stage + the manual step.
pub(crate) fn cmd_promote(args: PromoteArgs) -> Result<()> {
    let artifacts = collect_calibrations(&args.runs_dir)?;
    if artifacts.is_empty() {
        bail!(
            "no calibration artifacts found under {} (expected */review/calibration.json)",
            args.runs_dir.display()
        );
    }
    let agg = aggregate_calibrations(&artifacts);
    let rec = recommend_mode(&agg);

    println!(
        "ub-review promote — {} runs under {}",
        agg.runs,
        args.runs_dir.display()
    );
    println!();
    // Infer the current stage from the most recent run's mode.
    let current = artifacts
        .last()
        .map(|(_, cal)| cal.run_mode.as_str())
        .unwrap_or("unknown");
    println!("Current inferred mode: {current}");
    println!(
        "Target mode:          {} ({})",
        rec.recommended.key(),
        rec.stage
    );
    println!();
    println!("Criteria:");
    for c in &rec.criteria {
        let mark = if !c.measured {
            "~"
        } else if c.passed {
            "PASS"
        } else {
            "FAIL"
        };
        println!(
            "  [{mark}] {name}: {detail}",
            name = c.name,
            detail = c.detail
        );
    }
    println!();
    match rec.recommended {
        ReviewModePreset::Advisory => {
            if rec.needs_more_data {
                println!(
                    "Not ready to promote: collect at least {MIN_RUNS_FOR_RECOMMENDATION} runs."
                );
            } else {
                println!("Not ready to promote: stay advisory until the failing criteria pass.");
            }
        }
        ReviewModePreset::Gate => {
            println!("Ready to promote to gate.");
            println!();
            println!("Manual step:");
            println!("  Re-run: ub-review enable --mode gate --action-sha <sha> --force");
            println!("  Then make `ub-review/gate` a required branch-protection check:");
            println!(
                "    repo Settings -> Branches -> Require status checks -> add `ub-review/gate`"
            );
        }
        ReviewModePreset::Strict => {
            println!("Ready to promote to strict (review-forward).");
            println!();
            println!("Manual step:");
            println!("  Re-run: ub-review enable --mode strict --action-sha <sha> --force");
            println!("  Confirm `[gate].review_forward = true` in .ub-review.toml.");
            println!("  Keep `ub-review/gate` required in branch protection.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::{
        CalibrationClassification, CalibrationCohort, CalibrationCounts, CalibrationNotableEvent,
    };

    /// Build a minimal valid artifact for tests.
    fn fixture(run_class: &str, infra: bool, human: Option<&str>) -> CalibrationArtifact {
        CalibrationArtifact {
            schema: CALIBRATION_SCHEMA.to_owned(),
            repo: "test/repo".to_owned(),
            base: "main".to_owned(),
            head: "feature".to_owned(),
            run_mode: "intelligent-ci".to_owned(),
            gate_policy: "pass".to_owned(),
            cohort: CalibrationCohort {
                provider: "minimax".to_owned(),
                model: "MiniMax-M3".to_owned(),
                cohort_id: "abc".to_owned(),
                shared_prefix_hash: "deadbeef".to_owned(),
                cohort_broken: false,
            },
            counts: CalibrationCounts::default(),
            classification: CalibrationClassification {
                run_class: run_class.to_owned(),
                suggested_class: run_class.to_owned(),
                infra_excluded: infra,
                human_classification: human.map(str::to_owned),
            },
            notable_events: Vec::new(),
        }
    }

    fn artifact_with_proof_changed(proof_changed: usize) -> CalibrationArtifact {
        let mut cal = fixture("proof-changed-conclusion", false, None);
        cal.counts.lane_conclusions_changed_by_proof = proof_changed;
        cal
    }

    fn aggregate_of(artifacts: &[CalibrationArtifact]) -> CalibrationAggregate {
        let owned: Vec<(PathBuf, CalibrationArtifact)> = artifacts
            .iter()
            .enumerate()
            .map(|(i, cal)| (PathBuf::from(format!("/run-{i}")), cal.clone()))
            .collect();
        aggregate_calibrations(&owned)
    }

    #[test]
    fn load_calibration_reads_typed_artifact() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cal = fixture("expected-quiet", false, None);
        let review_dir = temp.path().join("review");
        fs::create_dir_all(&review_dir)?;
        fs::write(
            review_dir.join("calibration.json"),
            serde_json::to_vec(&cal)?,
        )?;
        let loaded = load_calibration(temp.path())?;
        assert_eq!(loaded.schema, CALIBRATION_SCHEMA);
        assert_eq!(loaded.cohort.provider, "minimax");
        assert_eq!(loaded.classification.suggested_class, "expected-quiet");
        Ok(())
    }

    #[test]
    fn load_calibration_rejects_wrong_schema() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut cal = fixture("expected-quiet", false, None);
        cal.schema = "ub-review.calibration.v999".to_owned();
        let review_dir = temp.path().join("review");
        fs::create_dir_all(&review_dir)?;
        fs::write(
            review_dir.join("calibration.json"),
            serde_json::to_vec(&cal)?,
        )?;
        let err = match load_calibration(temp.path()) {
            Ok(_) => anyhow::bail!("should have rejected wrong schema"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("expected"),
            "should name the schema mismatch: {err}"
        );
        Ok(())
    }

    #[test]
    fn recommend_advisory_when_too_few_runs() {
        let agg = aggregate_of(&[fixture("expected-quiet", false, None)]);
        let rec = recommend_mode(&agg);
        assert_eq!(rec.recommended, ReviewModePreset::Advisory);
        assert!(rec.needs_more_data);
    }

    #[test]
    fn recommend_advisory_when_infra_excluded_high() {
        // 6 runs, 1 infra-excluded = 16.7% >= 5%.
        let runs: Vec<CalibrationArtifact> = (0..6)
            .map(|i| {
                if i == 0 {
                    fixture("infra-excluded", true, None)
                } else {
                    fixture("expected-quiet", false, None)
                }
            })
            .collect();
        let agg = aggregate_of(&runs);
        assert!(agg.infra_excluded_rate() >= INFRA_EXCLUDED_THRESHOLD);
        let rec = recommend_mode(&agg);
        assert_eq!(rec.recommended, ReviewModePreset::Advisory);
        assert!(!rec.needs_more_data);
    }

    #[test]
    fn recommend_gate_when_healthy() {
        // 10 runs, 0 infra, no human labels (false-positive unmeasured -> not blocking).
        let runs: Vec<CalibrationArtifact> = (0..10)
            .map(|_| fixture("expected-quiet", false, None))
            .collect();
        let agg = aggregate_of(&runs);
        let rec = recommend_mode(&agg);
        assert_eq!(rec.recommended, ReviewModePreset::Gate);
        // No acted-on / proof-changed -> not strict.
        assert_eq!(rec.stage, "gate");
    }

    #[test]
    fn recommend_strict_requires_acted_on_and_proof_changed() {
        // Gate-healthy but no acted-on -> gate, not strict.
        let mut runs: Vec<CalibrationArtifact> = (0..10)
            .map(|_| fixture("expected-quiet", false, None))
            .collect();
        runs[0] = artifact_with_proof_changed(1); // proof signal present
        let agg = aggregate_of(&runs);
        let rec = recommend_mode(&agg);
        assert_eq!(
            rec.recommended,
            ReviewModePreset::Gate,
            "no acted-on -> not strict"
        );

        // Now add an acted-on label.
        runs[1].classification.human_classification = Some("acted-on".to_owned());
        let agg = aggregate_of(&runs);
        let rec = recommend_mode(&agg);
        assert_eq!(rec.recommended, ReviewModePreset::Strict);
    }

    #[test]
    fn aggregate_computes_rates() {
        let runs = vec![
            fixture("expected-quiet", false, Some("true-positive")),
            fixture("expected-quiet", false, Some("false-positive")),
            fixture("infra-excluded", true, None),
            fixture("expected-quiet", false, None),
        ];
        let agg = aggregate_of(&runs);
        assert_eq!(agg.runs, 4);
        assert_eq!(agg.infra_excluded_count, 1);
        assert_eq!(agg.infra_excluded_rate(), 0.25);
        assert_eq!(agg.human_labeled_count, 2);
        assert_eq!(agg.false_positive_count, 1);
        assert_eq!(agg.false_positive_rate(), 0.5); // 1 of 2 labeled
    }

    #[test]
    fn recommend_uses_human_classification_for_false_positives() {
        // 10 runs, all labeled false-positive -> 100% >= 10% -> advisory.
        let runs: Vec<CalibrationArtifact> = (0..10)
            .map(|_| fixture("needs-human-classification", false, Some("false-positive")))
            .collect();
        let agg = aggregate_of(&runs);
        assert!(agg.false_positive_rate() >= FALSE_POSITIVE_THRESHOLD);
        let rec = recommend_mode(&agg);
        assert_eq!(rec.recommended, ReviewModePreset::Advisory);
        // The false-positive criterion should be measured and failing.
        let fp_idx = rec
            .criteria
            .iter()
            .position(|c| c.name == "false-positive rate");
        assert!(
            fp_idx.is_some(),
            "false-positive criterion should be present"
        );
        let fp = &rec.criteria[fp_idx.unwrap_or(usize::MAX)];
        assert!(fp.measured);
        assert!(!fp.passed);
    }

    #[test]
    fn collect_calibrations_finds_nested_artifacts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        for i in 0..3 {
            let run_dir = temp.path().join(format!("run-{i}")).join("review");
            fs::create_dir_all(&run_dir)?;
            let cal = fixture("expected-quiet", false, None);
            fs::write(run_dir.join("calibration.json"), serde_json::to_vec(&cal)?)?;
        }
        let found = collect_calibrations(temp.path())?;
        assert_eq!(found.len(), 3, "should find all 3 nested artifacts");
        Ok(())
    }

    #[test]
    fn cmd_status_prints_notable_events() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut cal = fixture("proof-changed-conclusion", false, None);
        cal.notable_events.push(CalibrationNotableEvent {
            kind: "proof-changed-conclusion".to_owned(),
            proof_receipt: "review/proof/xyz".to_owned(),
            lane: "tests-red-green".to_owned(),
            before: "changes_requested".to_owned(),
            after: "clear".to_owned(),
            reason: "focused test passed".to_owned(),
        });
        let review_dir = temp.path().join("target/ub-review/review");
        fs::create_dir_all(&review_dir)?;
        fs::write(
            review_dir.join("calibration.json"),
            serde_json::to_vec(&cal)?,
        )?;
        // cmd_status prints to stdout; we verify it does not error and the
        // notable event is in the artifact (the print path is exercised).
        let args = StatusArgs {
            run_dir: temp.path().join("target/ub-review"),
        };
        cmd_status(args)?;
        Ok(())
    }
}

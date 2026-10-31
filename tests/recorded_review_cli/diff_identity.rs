//! #1547: mandatory diff custody survives optional lead/plan states.
use super::*;

const CHANGED: &[u8] = b"diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-safe();\n+changed();\n";

fn accepted(output: Output) -> Result<Vec<u8>> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty());
    Ok(output.stdout)
}

fn replay(f: &Fixture, view: &[u8]) -> Result<Output> {
    fs::write(f.root.path().join("view.json"), view)?;
    f.invoke(
        "replay",
        &["--view", "view.json", "--view-sha256", &hash(view)],
    )
}

fn contradictory_diff(state: &str, lead: &str) -> Result<()> {
    let mut f = Fixture::new()?;
    if state != "validated" {
        f.intake["plan"] = Value::Null;
    }
    if state == "lead_absent" {
        f.intake["answer"] = Value::Null;
    } else if state == "lead_invalid" {
        let invalid = b"private invalid lead";
        fs::write(f.root.path().join("answer.json"), invalid)?;
        f.intake["answer"]["sha256"] = json!(hash(invalid));
    }
    let ci = fs::read(f.root.path().join("ci.bin"))?;
    let original = accepted(f.invoke("export", &[])?)?;
    let view: Value = serde_json::from_slice(&original)?;
    assert_eq!(view["judgment"]["availability"], lead);
    assert_eq!(view["public_plan"]["availability"], state);
    assert_eq!(view["source_currentness"], "matches_admitted_revision");
    assert_eq!(view["qualification"], "shadow_recorded_only");
    assert_eq!(view["delivery_projection"], "not_admitted");
    assert_eq!(view["remote_effects"], "unknown");
    assert_eq!(view["imported_ci_digest"], hash(&ci));
    assert!(!String::from_utf8_lossy(&original).contains("private invalid"));
    let receipt: Value = serde_json::from_slice(&accepted(replay(&f, &original)?)?)?;
    assert_eq!(receipt["status"], "replayed_recorded_sources");
    assert_eq!(receipt["view_sha256"], hash(&original));

    fs::write(f.root.path().join("diff.patch"), CHANGED)?;
    rejected(f.invoke("export", &[])?, "SHA256 mismatch");
    rejected(replay(&f, &original)?, "SHA256 mismatch");
    f.intake["diff"]["sha256"] = json!(hash(CHANGED));
    // invoke independently pins the new intake; old-pin rejection cannot mask
    // acceptance of contradictory diff bytes at the canonical boundary.
    let changed_export = f.invoke("export", &[])?;
    let changed_replay = replay(&f, &original)?;
    eprintln!(
        "{state}: repinned export={} stdout={} replay={} stdout={}",
        changed_export.status,
        changed_export.stdout.len(),
        changed_replay.status,
        changed_replay.stdout.len()
    );
    rejected(changed_export, "source validation rejected");
    rejected(changed_replay, "replay rejected");

    fs::write(f.root.path().join("diff.patch"), PATCH)?;
    f.intake["diff"]["sha256"] = json!(hash(PATCH));
    assert_eq!(accepted(f.invoke("export", &[])?)?, original);
    accepted(replay(&f, &original)?)?;
    assert_eq!(fs::read(f.root.path().join("ci.bin"))?, ci);
    Ok(())
}

#[test]
fn plan_present_rejects_contradictory_diff() -> Result<()> {
    contradictory_diff("validated", "validated")
}

#[test]
fn plan_absent_rejects_contradictory_diff() -> Result<()> {
    contradictory_diff("plan_absent", "validated")
}

#[test]
fn lead_absent_rejects_contradictory_diff() -> Result<()> {
    contradictory_diff("lead_absent", "absent")
}

#[test]
fn lead_invalid_rejects_contradictory_diff() -> Result<()> {
    contradictory_diff("lead_invalid", "invalid")
}

#[test]
fn regenerated_diff_revision_and_sources_replay_only_their_own_view() -> Result<()> {
    let mut old = Fixture::new()?;
    old.intake["plan"] = Value::Null;
    let old_view = accepted(old.invoke("export", &[])?)?;
    let mut fresh = Fixture::with_patch(CHANGED)?;
    fresh.intake["plan"] = Value::Null;
    let fresh_view = accepted(fresh.invoke("export", &[])?)?;
    let old_value: Value = serde_json::from_slice(&old_view)?;
    let fresh_value: Value = serde_json::from_slice(&fresh_view)?;
    assert_ne!(
        old_value["current_revision"],
        fresh_value["current_revision"]
    );
    assert_ne!(
        old_value["source_manifest_digest"],
        fresh_value["source_manifest_digest"]
    );
    assert_eq!(fresh_value["judgment"]["availability"], "validated");
    assert_eq!(fresh_value["public_plan"]["availability"], "plan_absent");
    assert_eq!(
        fresh_value["imported_ci_digest"],
        old_value["imported_ci_digest"]
    );
    accepted(replay(&fresh, &fresh_view)?)?;
    rejected(replay(&fresh, &old_view)?, "replay rejected");
    rejected(replay(&old, &fresh_view)?, "replay rejected");
    assert_eq!(accepted(old.invoke("export", &[])?)?, old_view);
    accepted(replay(&old, &old_view)?)?;
    Ok(())
}

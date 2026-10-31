use anyhow::{Context, Result, ensure};
use std::{path::Path, process::Command};

// Preparatory source installation tests the dormant exact cutover fixtures.
// Each cutover explicitly selects its installed lanes; uninstalled lanes use fixtures.
const ACTIVE_LANES: &[&str] = &[];

#[test]
fn native_admission_helper_contract_is_exercised_without_network_or_product_proof() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (lane, name) in [
        ("gate", "ub-review-gate.yml"),
        ("baseline", "independent-baseline.yml"),
    ] {
        let active = std::fs::read(root.join(".github/workflows").join(name))?;
        let fixture = std::fs::read(root.join("fixtures/premerge-admission").join(name))?;
        if !ACTIVE_LANES.contains(&lane) {
            ensure!(
                active != fixture,
                "preparation must not install the dormant workflow"
            );
        } else {
            ensure!(
                active == fixture,
                "cutover workflow differs from the prepared exact fixture"
            );
        }
    }
    let output = Command::new("python3")
        .args(["-B", "scripts/test-premerge-admission.py"])
        .env("UB_REVIEW_ADMISSION_ACTIVE_LANES", ACTIVE_LANES.join(","))
        .current_dir(root)
        .output()
        .context("execute the offline production-helper controls")?;
    ensure!(
        output.status.success(),
        "offline admission controls failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        String::from_utf8_lossy(&output.stderr).contains("Ran 28 tests"),
        "the complete offline admission control inventory must run"
    );
    Ok(())
}

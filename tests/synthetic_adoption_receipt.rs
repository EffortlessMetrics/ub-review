//! Offline receipt control for #1346. All identities describe inert fixture data.

use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[test]
fn synthetic_receipt_requires_exact_previous_state_after_rollback() -> Result<()> {
    let provenance = "synthetic_adoption_fixture";
    let fixture = "release-adoption/owned-files/v1";
    let sha256 = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let snapshot = |release, archive, binary, config, workflow| {
        let archive_sha256 = sha256(archive);
        let checksum = format!("{archive_sha256}  fixture-archive\n");
        json!({
            "release": release,
            "archive_sha256": archive_sha256,
            "checksum_sha256": sha256(checksum.as_bytes()),
            "binary_sha256": sha256(binary),
            "config_sha256": sha256(config),
            "workflow_sha256": sha256(workflow)
        })
    };
    let previous = snapshot(
        "v0.0.0-fixture.1",
        b"fixture archive 1\n",
        b"inert executable bytes 1\n",
        b"mode = \"advisory\"\nversion = 1\n",
        b"name: Synthetic advisory 1\n",
    );
    let next = snapshot(
        "v0.0.0-fixture.2",
        b"fixture archive 2\n",
        b"inert executable bytes 2\n",
        b"mode = \"advisory\"\nversion = 2\n",
        b"name: Synthetic advisory 2\n",
    );
    let input_sha256 = sha256(b"frozen advisory input\n");
    let existing_ci_sha256 = sha256(b"name: Existing CI\n");
    let observe = |state: &Value| {
        json!({
            "provenance": provenance,
            "fixture": fixture,
            "input_sha256": input_sha256,
            "existing_ci_sha256": existing_ci_sha256,
            "state": state
        })
    };
    let receipt = json!({
        "install": observe(&previous),
        "upgrade": observe(&next),
        "rollback": observe(&previous)
    });
    let verify = |receipt: &Value| -> std::result::Result<(), String> {
        for (stage, expected) in [
            ("install", &previous),
            ("upgrade", &next),
            ("rollback", &previous),
        ] {
            let observed = &receipt[stage];
            if observed["provenance"] != provenance || observed["fixture"] != fixture {
                return Err(format!("{stage}: synthetic fixture identity mismatch"));
            }
            if observed["input_sha256"] != input_sha256
                || observed["existing_ci_sha256"] != existing_ci_sha256
            {
                return Err(format!(
                    "{stage}: frozen input or existing CI identity mismatch"
                ));
            }
            if &observed["state"] != expected {
                return Err(format!("{stage}: snapshot identity mismatch"));
            }
        }
        Ok(())
    };
    assert_eq!(verify(&receipt), Ok(()));
    let receipt_json = serde_json::to_string(&receipt)?;
    println!(
        "synthetic receipt sha256={}",
        sha256(receipt_json.as_bytes())
    );
    println!("{receipt_json}");

    // Old release and binary identities are insufficient if the new config survives.
    let mut stale_config = receipt.clone();
    stale_config["rollback"]["state"]["config_sha256"] = next["config_sha256"].clone();
    assert_eq!(
        verify(&stale_config).err().as_deref(),
        Some("rollback: snapshot identity mismatch")
    );

    let mut candidate = receipt;
    candidate["upgrade"]["provenance"] = json!("candidate_rehearsal");
    assert_eq!(
        verify(&candidate).err().as_deref(),
        Some("upgrade: synthetic fixture identity mismatch")
    );
    Ok(())
}

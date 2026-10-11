//! #1066 preparatory packet audit only. This is not runtime admission or a
//! public-install/compatibility verifier; there is no selected positive case.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

const PACKET: &str = include_str!("../fixtures/analyzer-selection/1066-blocked.json");

fn canonical_payload(packet: &Value) -> Result<Value> {
    let mut payload = packet["payload"].clone();
    for (field, key) in [("tools", "id"), ("mirrors", "path"), ("fixtures", "path")] {
        let rows = payload[field].as_array_mut().context("missing inventory")?;
        rows.sort_by(|a, b| a[key].as_str().cmp(&b[key].as_str()));
    }
    Ok(payload)
}

fn digest(packet: &Value) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical_payload(packet)?)?)
    ))
}

fn seal(packet: &mut Value) -> Result<()> {
    let hash = digest(packet)?;
    packet["payload_sha256"] = hash.clone().into();
    packet["generation"] = format!("1066-preparatory-blocked-{hash}").into();
    Ok(())
}

fn audit(packet: &Value) -> Result<()> {
    ensure!(
        packet["payload_sha256"] == digest(packet)?,
        "packet digest mismatch"
    );
    ensure!(
        packet["generation"] == format!("1066-preparatory-blocked-{}", digest(packet)?),
        "generation mismatch"
    );
    let p = &packet["payload"];
    ensure!(
        p["selection_status"] == "not_selected" && p["required_readiness"] == false,
        "blocked scope changed"
    );
    ensure!(
        p["policy"]["max_new_unsuppressed"] == 0,
        "strict-zero changed"
    );
    ensure!(
        p["source_candidate"]["selected"] == false,
        "source promoted"
    );
    let config: toml::Value = toml::from_str(include_str!("../.ub-review.toml"))?;
    let registry = config["tools"].as_table().context("tools registry")?;
    let tools = p["tools"].as_array().context("tool rows")?;
    let expected: Vec<_> = registry
        .iter()
        .filter(|(_, row)| row.get("enabled").and_then(toml::Value::as_bool) != Some(false))
        .map(|(id, _)| id.as_str())
        .collect();
    let mut ids: Vec<_> = tools
        .iter()
        .map(|row| row["id"].as_str().context("tool id"))
        .collect::<Result<_>>()?;
    ids.sort_unstable();
    ensure!(ids == expected, "missing/duplicate/unknown row");
    for row in tools {
        let id = row["id"].as_str().context("id")?;
        let required = registry[id]
            .get("required")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        ensure!(
            row["role"] == if required { "required" } else { "advisory" },
            "role drift"
        );
        ensure!(
            row["disposition"] == "blocked_missing_public_identity",
            "selection claim in blocked packet"
        );
        ensure!(
            !row["missing_evidence"]
                .as_array()
                .context("missing evidence")?
                .is_empty(),
            "missing blockers"
        );
        ensure!(
            row["candidate"]["source"].is_null()
                && row["candidate"]["checksum"].is_null()
                && row["candidate"]["install_receipt"].is_null()
                && row["candidate"]["compatibility_receipt"].is_null(),
            "unverified candidate evidence introduced"
        );
        ensure!(
            row["rollback"]["qualified"] == false
                && row["rollback"]["compatibility_receipt"].is_null()
                && row["rollback"]["install_receipt"].is_null(),
            "rollback falsely qualified"
        );
        ensure!(
            row["1067_mirror_changes"]
                .as_array()
                .context("mirror changes")?
                .is_empty(),
            "migration authorized"
        );
    }
    let ripr = tools.iter().find(|r| r["id"] == "ripr").context("ripr")?;
    ensure!(
        ripr["current"]["version"] == "0.10.0"
            && ripr["current"]["checksum"] == p["public_observations"]["crate_sha256"]
            && ripr["current"]["source_sha"] == p["public_observations"]["crate_vcs_sha"],
        "stable public identity conflict"
    );
    ensure!(
        p["public_observations"]["candidate_0_11_0_present"] == false
            && p["public_observations"]["clean_install"] == "not_run",
        "public probe claim changed"
    );
    Ok(())
}

#[test]
fn packet_is_blocked_and_sealed_against_frozen_mirrors() -> Result<()> {
    let packet: Value = serde_json::from_str(PACKET)?;
    audit(&packet)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for field in ["mirrors", "fixtures"] {
        for row in packet["payload"][field].as_array().context("inventory")? {
            let path = row["path"].as_str().context("path")?;
            ensure!(
                !path.starts_with('/') && !path.split('/').any(|s| s == ".."),
                "unsafe inventory path"
            );
            let actual = format!("{:x}", Sha256::digest(std::fs::read(root.join(path))?));
            ensure!(row["sha256"] == actual, "frozen inventory drift: {path}");
        }
    }
    Ok(())
}

#[test]
fn inventory_order_and_checkout_root_do_not_change_generation() -> Result<()> {
    let packet: Value = serde_json::from_str(PACKET)?;
    let mut reordered = packet.clone();
    for field in ["tools", "mirrors", "fixtures"] {
        reordered["payload"][field]
            .as_array_mut()
            .context("inventory")?
            .reverse();
    }
    ensure!(
        digest(&packet)? == digest(&reordered)?,
        "order-dependent digest"
    );
    audit(&reordered)?;
    // Canonical inputs contain repository-relative paths only; no checkout root.
    ensure!(
        !PACKET.contains("/workspace") && !PACKET.contains("/tmp/"),
        "root leaked into packet"
    );
    Ok(())
}

#[test]
fn resealed_invalid_selection_claims_are_rejected() -> Result<()> {
    let packet: Value = serde_json::from_str(PACKET)?;
    let index = packet["payload"]["tools"]
        .as_array()
        .context("tools")?
        .iter()
        .position(|r| r["id"] == "ripr")
        .context("ripr")?;
    for case in [
        "source_only",
        "wrong_digest",
        "missing_compatibility",
        "incompatible_rollback",
        "unknown_as_zero",
        "missing_row",
        "threshold",
        "readiness",
    ] {
        let mut invalid = packet.clone();
        match case {
            "source_only" => {
                invalid["payload"]["tools"][index]["candidate"]["source"] =
                    "workspace/path binary".into();
                invalid["payload"]["tools"][index]["disposition"] = "selected_required".into();
            }
            "wrong_digest" => {
                invalid["payload"]["tools"][index]["current"]["checksum"] =
                    "wrong digest with version 0.10.0".into();
            }
            "missing_compatibility" => {
                invalid["payload"]["tools"][index]["disposition"] = "selected_required".into();
            }
            "incompatible_rollback" => {
                invalid["payload"]["tools"][index]["rollback"]["qualified"] = true.into();
            }
            "unknown_as_zero" => {
                invalid["payload"]["tools"][index]["candidate"]["compatibility_receipt"] = 0.into();
            }
            "missing_row" => {
                invalid["payload"]["tools"]
                    .as_array_mut()
                    .context("tools")?
                    .remove(index);
            }
            "threshold" => {
                invalid["payload"]["policy"]["max_new_unsuppressed"] = 1.into();
            }
            "readiness" => {
                invalid["payload"]["required_readiness"] = true.into();
            }
            _ => anyhow::bail!("unknown case"),
        }
        seal(&mut invalid)?;
        ensure!(audit(&invalid).is_err(), "packet audit admitted {case}");
    }
    Ok(())
}

#[test]
fn tampering_without_new_generation_is_rejected() -> Result<()> {
    let mut packet: Value = serde_json::from_str(PACKET)?;
    packet["payload"]["source_candidate"]["revision_hint"] = "different source".into();
    ensure!(audit(&packet).is_err(), "unsealed tamper admitted");
    Ok(())
}

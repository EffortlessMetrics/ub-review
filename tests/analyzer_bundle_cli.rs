//! Real CLI controls with synthetic recorded evidence. No archive is an actual
//! public analyzer and no fixture establishes installation or release acceptance.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::process::Command;

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn write(root: &std::path::Path, path: &str, bytes: &[u8]) -> Result<Value> {
    fs::write(root.join(path), bytes)?;
    Ok(json!({"path":path,"sha256":hash(bytes)}))
}
fn write_json(root: &std::path::Path, name: &str, value: &Value) -> Result<Value> {
    write(root, &format!("{name}.json"), &serde_json::to_vec(value)?)
}

struct Fixture {
    dir: tempfile::TempDir,
    intake: Value,
    bundle: Value,
    records: BTreeMap<String, Value>,
}

impl Fixture {
    fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let scope = json!({"revision":{"digest":"a".repeat(64),"semantics":"candidate_head","reviewed_commit":"b".repeat(40)},"corpus_sha256":"c".repeat(64),"policy_sha256":"d".repeat(64),"suppression_sha256":"e".repeat(64),"platform":"x86_64-unknown-linux-gnu"});
        let upstream = write(
            root,
            "source-owned-validation.txt",
            b"SYNTHETIC reviewed source-owned package install acceptance; not real publication",
        )?;
        let mut identities = Vec::new();
        let mut records = BTreeMap::new();
        for (side, version) in [("candidate", "2.0.0"), ("rollback", "1.0.0")] {
            let artifact = write(
                root,
                &format!("{side}.crate"),
                format!("synthetic {side} package snapshot").as_bytes(),
            )?;
            let executable = write(
                root,
                &format!("{side}.bin"),
                format!("synthetic {side} executable snapshot; never executed").as_bytes(),
            )?;
            let stdout = write(
                root,
                &format!("{side}-version.txt"),
                format!("fixture-analyzer {version}\n").as_bytes(),
            )?;
            let capabilities = write(
                root,
                &format!("{side}-capabilities.txt"),
                b"fixture-schema1 capabilities snapshot\n",
            )?;
            let source = if side == "candidate" {
                "1".repeat(40)
            } else {
                "2".repeat(40)
            };
            let identity = json!({"package":"fixture-analyzer","version":version,"source_url":format!("https://static.crates.io/crates/fixture-analyzer/fixture-analyzer-{version}.crate"),"source_sha":source,"tag_sha":source,"artifact":artifact,"executable":executable,"output_schema":"fixture.schema1","capability_sha256":capabilities["sha256"],"probes":[{"argv":["--version"],"stdout_sha256":stdout["sha256"]},{"argv":["capabilities","--json"],"stdout_sha256":capabilities["sha256"]}]});
            records.insert(format!("{side}-public"),json!({"schema":"ub-review.analyzer_public_identity.v1","owner":"fixture-public-owner","tool_id":"fixture-analyzer","package":"fixture-analyzer","version":version,"source_url":identity["source_url"],"source_sha":source,"tag":format!("v{version}"),"tag_sha":source,"artifact_sha256":artifact["sha256"],"public_verified":true}));
            records.insert(format!("{side}-install"),json!({"schema":"ub-review.analyzer_install_probe.v1","owner":"fixture-install-owner","package_validator_source":"3".repeat(40),"upstream_validation":upstream,"tool_id":"fixture-analyzer","version":version,"scope":scope,"origin":"public_registry","source_url":identity["source_url"],"artifact_sha256":artifact["sha256"],"executable_sha256":executable["sha256"],"clean_prefix":true,"source_checkout_used":false,"install_argv":["cargo","install","fixture-analyzer","--version",version,"--locked","--root","<clean-prefix>"],"output_schema":"fixture.schema1","capability_sha256":capabilities["sha256"],"probes":[{"argv":["--version"],"executable_sha256":executable["sha256"],"exit_code":0,"stdout":stdout},{"argv":["capabilities","--json"],"executable_sha256":executable["sha256"],"exit_code":0,"stdout":capabilities}]}));
            identities.push(identity);
        }
        for (side, index) in [("candidate", 0), ("rollback", 1)] {
            records.insert(format!("{side}-compatibility"),json!({"schema":"ub-review.analyzer_compatibility.v1","owner":"fixture-compatibility-owner","tool_id":"fixture-analyzer","scope":scope,"candidate_artifact_sha256":identities[index]["artifact"]["sha256"],"candidate_executable_sha256":identities[index]["executable"]["sha256"],"baseline_artifact_sha256":identities[1]["artifact"]["sha256"],"decision":"accepted","rollback_qualified":true,"rollback_from_artifact_sha256":identities[0]["artifact"]["sha256"]}));
        }
        let resolved = write_json(
            root,
            "resolved-tools",
            &json!({"schema":"ub-review.resolved_tools.v1","runtime_profile":"fixture","tools":[{"id":"fixture-analyzer","enabled":true,"required":true}]}),
        )?;
        let bundle = json!({"schema":"ub-review.analyzer_bundle.v1","scope":scope,"tools":[{"id":"fixture-analyzer","required":true,"disposition":"selected_required","candidate":identities[0],"rollback":identities[1]}]});
        let intake = json!({"schema":"ub-review.analyzer_selection_intake.v1","scope":scope,"resolved_tools":resolved,"bundle":null,"public_identity_owner":"fixture-public-owner","install_probe_owner":"fixture-install-owner","compatibility_owner":"fixture-compatibility-owner","tools":[{"id":"fixture-analyzer","supported_output_schemas":["fixture.schema1"],"supported_platforms":["x86_64-unknown-linux-gnu"],"package_validator_source":"3".repeat(40),"current_released":identities[1],"candidate":{"public_identity":null,"install_probe":null,"compatibility":null},"rollback":{"public_identity":null,"install_probe":null,"compatibility":null}}]});
        Ok(Self {
            dir,
            intake,
            bundle,
            records,
        })
    }

    fn run(&mut self, missing: bool) -> Result<(bool, Value)> {
        for side in ["candidate", "rollback"] {
            for (suffix, key) in [
                ("public", "public_identity"),
                ("install", "install_probe"),
                ("compatibility", "compatibility"),
            ] {
                let name = format!("{side}-{suffix}");
                self.intake["tools"][0][side][key] =
                    write_json(self.dir.path(), &name, &self.records[&name])?;
            }
        }
        if missing {
            self.intake["tools"][0]["candidate"]["compatibility"] = Value::Null;
        }
        self.intake["bundle"] = write_json(self.dir.path(), "bundle", &self.bundle)?;
        let reference = write_json(self.dir.path(), "intake", &self.intake)?;
        let output = Command::new(env!("CARGO_BIN_EXE_ub-review"))
            .args([
                "verify-analyzer-bundle",
                "--intake",
                "intake.json",
                "--intake-sha256",
                reference["sha256"].as_str().context("intake digest")?,
                "--evidence-root",
            ])
            .arg(self.dir.path())
            .output()?;
        let receipt: Value = serde_json::from_slice(&output.stdout)
            .with_context(|| String::from_utf8_lossy(&output.stderr).into_owned())?;
        ensure!(
            receipt["schema"] == "ub-review.analyzer_selection_receipt.v1"
                && receipt["required_readiness"] == false,
            "receipt boundary"
        );
        ensure!(
            receipt["intake_sha256"] == hash(&fs::read(self.dir.path().join("intake.json"))?),
            "intake identity"
        );
        Ok((output.status.success(), receipt))
    }
}

#[test]
fn fully_bound_recorded_selection_is_valid_but_never_required_ready() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let (success, receipt) = fixture.run(false)?;
    ensure!(
        success && receipt["status"] == "selected_recorded_receipt",
        "recorded positive: {receipt}"
    );
    ensure!(
        receipt["validated_tools"] == json!(["fixture-analyzer"]),
        "tool identity"
    );
    ensure!(
        receipt["bundle_sha256"] == hash(&fs::read(fixture.dir.path().join("bundle.json"))?),
        "independent bundle hash"
    );
    ensure!(
        receipt["claim_boundary"]
            .as_str()
            .context("claim")?
            .contains("Producer authenticity"),
        "public qualification inferred"
    );
    Ok(())
}

#[test]
fn actual_entrypoint_rejects_wrong_missing_stale_and_unqualified_evidence() -> Result<()> {
    for (case, status, reason) in [
        (
            "artifact",
            "contradictory_evidence",
            "evidence digest mismatch",
        ),
        (
            "executable",
            "contradictory_evidence",
            "evidence digest mismatch",
        ),
        (
            "source",
            "contradictory_evidence",
            "public artifact/source/tag contradiction",
        ),
        (
            "tag",
            "contradictory_evidence",
            "public artifact/source/tag contradiction",
        ),
        (
            "tool",
            "contradictory_evidence",
            "wrong tool/install receipt",
        ),
        (
            "probe",
            "contradictory_evidence",
            "wrong executable/probe contract",
        ),
        ("stale", "stale_evidence", "receipt scope differs"),
        (
            "missing",
            "missing_evidence",
            "missing compatibility receipt pin",
        ),
        (
            "rollback",
            "not_selected",
            "compatibility or rollback qualification",
        ),
        (
            "incompatible",
            "not_selected",
            "compatibility or rollback qualification",
        ),
        (
            "workspace",
            "contradictory_evidence",
            "workspace substitution",
        ),
        (
            "schema",
            "contradictory_evidence",
            "unsupported output schema",
        ),
        ("platform", "contradictory_evidence", "unsupported platform"),
        ("version_only", "contradictory_evidence", "version alone"),
        ("unknown", "contradictory_evidence", "unknown field"),
        (
            "not_selected",
            "not_selected",
            "blocked_missing_public_identity",
        ),
        ("inventory", "contradictory_evidence", "inventory mismatch"),
        (
            "rollback_transition",
            "contradictory_evidence",
            "rollback transition differs",
        ),
        (
            "validator",
            "contradictory_evidence",
            "package identity validator provenance",
        ),
    ] {
        let mut f = Fixture::new()?;
        match case {
            "artifact" => fs::write(
                f.dir.path().join("candidate.crate"),
                b"wrong digest; same claimed version",
            )?,
            "executable" => fs::write(
                f.dir.path().join("candidate.bin"),
                b"workspace substitute bytes",
            )?,
            "source" => {
                f.records.get_mut("candidate-public").context("public")?["source_sha"] =
                    "9".repeat(40).into()
            }
            "tag" => {
                f.records.get_mut("candidate-public").context("public")?["tag_sha"] =
                    "9".repeat(40).into()
            }
            "tool" => {
                f.records.get_mut("candidate-install").context("install")?["tool_id"] =
                    "another-tool".into()
            }
            "probe" => {
                f.records.get_mut("candidate-install").context("install")?["probes"][1]["argv"] =
                    json!(["different-command"])
            }
            "stale" => {
                f.records
                    .get_mut("candidate-compatibility")
                    .context("compatibility")?["scope"]["corpus_sha256"] = "9".repeat(64).into()
            }
            "missing" => {}
            "rollback" => {
                f.records
                    .get_mut("rollback-compatibility")
                    .context("compatibility")?["rollback_qualified"] = false.into()
            }
            "incompatible" => {
                f.records
                    .get_mut("rollback-compatibility")
                    .context("compatibility")?["decision"] = "incompatible".into()
            }
            "workspace" => {
                f.records.get_mut("candidate-install").context("install")?["origin"] =
                    "workspace".into()
            }
            "schema" => {
                f.bundle["tools"][0]["candidate"]["output_schema"] = "unsupported.schema9".into()
            }
            "platform" => {
                f.intake["tools"][0]["supported_platforms"] = json!(["unsupported-other-host"])
            }
            "version_only" => {
                f.bundle["tools"][0]["candidate"]["probes"]
                    .as_array_mut()
                    .context("probes")?
                    .pop();
            }
            "unknown" => {
                f.records.get_mut("candidate-public").context("public")?["authority_override"] =
                    true.into()
            }
            "not_selected" => {
                f.bundle["tools"][0]["disposition"] = "blocked_missing_public_identity".into()
            }
            "inventory" => {
                f.bundle["tools"].as_array_mut().context("rows")?.clear();
            }
            "rollback_transition" => {
                f.records
                    .get_mut("rollback-compatibility")
                    .context("compatibility")?["rollback_from_artifact_sha256"] =
                    "9".repeat(64).into();
            }
            "validator" => {
                f.records.get_mut("candidate-install").context("install")?["package_validator_source"] =
                    "9".repeat(40).into()
            }
            _ => anyhow::bail!("unknown case"),
        }
        let (success, receipt) = f.run(case == "missing")?;
        ensure!(
            !success && receipt["status"] == status && receipt["validated_tools"] == json!([]),
            "{case}: {receipt}"
        );
        ensure!(
            receipt["reason"]
                .as_str()
                .context("reason")?
                .contains(reason),
            "wrong rejection for {case}: {receipt}"
        );
    }
    Ok(())
}

//! Independent #1398 negative controls. These consume recorded fixtures only.
use super::*;

const AUTHORITY_FIELDS: [&str; 6] = [
    "tests_executed",
    "tests_passed",
    "ci_evidence_result",
    "required",
    "dispatch",
    "argv",
];

fn put(f: &Fixture, name: &str, value: &Value) -> Result<Value> {
    write(f, name, &serde_json::to_vec(value)?)
}

fn reseal(f: &mut Fixture) -> Result<()> {
    let source = domain_hash(
        "ub-review.recorded-judgment-source.v1",
        f.source_json.as_bytes(),
    );
    f.intake["source_manifest_digest"] = json!(source);
    let mut answer: Value = serde_json::from_slice(&fs::read(f.root.path().join("answer.json"))?)?;
    answer["source_manifest_digest"] = json!(source);
    f.intake["answer"] = put(f, "answer.json", &answer)?;
    let mut plan: Value = serde_json::from_slice(&fs::read(f.root.path().join("plan.json"))?)?;
    plan["judgment_digest"] = f.intake["answer"]["sha256"].clone();
    f.intake["plan"] = put(f, "plan.json", &plan)?;
    Ok(())
}

fn set_ci(f: &mut Fixture, ci: Option<&[u8]>) -> Result<()> {
    // Preserve the independently frozen producer field order in source_json.
    let offset = f
        .source_json
        .rfind(",\"imported_ci\":")
        .ok_or_else(|| anyhow::anyhow!("fixture CI field absent"))?;
    f.source_json.truncate(offset);
    let imported = if let Some(ci) = ci {
        f.intake["imported_ci"] = write(f, "ci.bin", ci)?;
        format!("{{\"digest\":\"{}\"}}", hash(ci))
    } else {
        f.intake["imported_ci"] = Value::Null;
        fs::remove_file(f.root.path().join("ci.bin"))?;
        "null".to_owned()
    };
    f.source_json
        .push_str(&format!(",\"imported_ci\":{imported}}}"));
    reseal(f)
}

fn clean(f: &mut Fixture) -> Result<()> {
    let graph = fs::read_to_string(f.root.path().join("graph.json"))?;
    let refuted = graph.replace("\"state\":\"Confirmed\"", "\"state\":\"Refuted\"");
    assert_ne!(refuted, graph);
    f.source_json = f.source_json.replace(&graph, &refuted);
    f.intake["graph"] = write(f, "graph.json", refuted.as_bytes())?;
    reseal(f)?;
    let mut answer: Value = serde_json::from_slice(&fs::read(f.root.path().join("answer.json"))?)?;
    answer["review_result"] = json!("clean");
    answer["selections"] = json!([]);
    f.intake["answer"] = put(f, "answer.json", &answer)?;
    let mut plan: Value = serde_json::from_slice(&fs::read(f.root.path().join("plan.json"))?)?;
    plan["judgment_digest"] = f.intake["answer"]["sha256"].clone();
    plan["quiet"] = json!(true);
    plan["items"] = json!([]);
    f.intake["plan"] = put(f, "plan.json", &plan)?;
    Ok(())
}

fn history(ci: Option<&[u8]>) -> Result<History> {
    let mut h = History::new()?;
    let keys = ["source_manifest_digest", "answer", "plan"];
    let before = keys.map(|key| {
        if key == "source_manifest_digest" {
            h.f.intake[key].clone()
        } else {
            h.f.intake[key]["sha256"].clone()
        }
    });
    set_ci(&mut h.f, ci)?;
    for (key, old) in keys.into_iter().zip(before) {
        let new = if key == "source_manifest_digest" {
            &h.f.intake[key]
        } else {
            &h.f.intake[key]["sha256"]
        };
        let old = old
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("old pin absent"))?;
        let new = new
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("new pin absent"))?;
        h.subject_json = h.subject_json.replace(old, new);
        h.payload_metadata_json = h.payload_metadata_json.replace(old, new);
    }
    h.subject = serde_json::from_str(&h.subject_json)?;
    h.f.intake["observations"]["payload_source_digest"] = json!(domain_hash(
        "ub-review.recorded-prepared-payload-source.v1",
        h.payload_metadata_json.as_bytes()
    ));
    h.rechain()?;
    h.admit(1)?;
    Ok(h)
}

fn source_intake(f: &mut Fixture) {
    f.intake["schema"] = json!("ub-review.recorded-file-input.v1");
    f.intake.as_object_mut().map(|m| m.remove("observations"));
}

fn no_owned_authority(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (name, child) in fields {
                assert!(
                    !AUTHORITY_FIELDS.contains(&name.as_str()),
                    "owned authority field: {name}"
                );
                assert!(
                    !["ci_result", "gate_result", "check_exit_status"].contains(&name.as_str()),
                    "owned result: {name}"
                );
                no_owned_authority(child);
            }
        }
        Value::Array(items) => items.iter().for_each(no_owned_authority),
        _ => {}
    }
}

fn successful(output: Output) -> Result<Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout)?;
    no_owned_authority(&value);
    // Byte arrays are opaque on the wire; inspect the two component envelopes too.
    for name in ["source_view_json", "observation_view_json"] {
        if !value[name].is_null() {
            no_owned_authority(&decode_bytes(&value[name])?);
        }
    }
    Ok(value)
}

fn export_replay(f: &Fixture, agent: bool) -> Result<Value> {
    let operation = if agent { "agent-export" } else { "export" };
    let out = f.invoke(operation, &[])?;
    let view = successful(Output {
        status: out.status,
        stdout: out.stdout.clone(),
        stderr: out.stderr,
    })?;
    fs::write(f.root.path().join("authority-view.json"), &out.stdout)?;
    let replay = f.invoke(
        if agent { "agent-replay" } else { "replay" },
        &[
            "--view",
            "authority-view.json",
            "--view-sha256",
            &hash(&out.stdout),
        ],
    )?;
    successful(replay)?;
    Ok(view)
}

#[test]
fn clean_and_findings_never_acquire_ci_authority_when_ci_is_absent_or_adverse() -> Result<()> {
    for ci in [
        None,
        Some(b"{\"result\":\"fail\"}\n".as_slice()),
        Some(b"{\"result\":\"not_proven\"}\n\0\xff".as_slice()),
    ] {
        let mut f = Fixture::new()?;
        set_ci(&mut f, ci)?;
        clean(&mut f)?;
        let clean = export_replay(&f, false)?;
        assert_eq!(clean["judgment"]["judgment"]["review_result"], "clean");
        assert_eq!(clean["public_plan"]["plan"]["quiet"], true);
        assert_eq!(
            clean["imported_ci_digest"],
            ci.map(hash).map_or(Value::Null, Value::String)
        );
        let mut h = history(ci)?;
        for agent in [true, false] {
            if !agent {
                source_intake(&mut h.f);
            }
            let view = export_replay(&h.f, agent)?;
            let source = if agent {
                decode_bytes(&view["source_view_json"])?
            } else {
                view
            };
            assert_eq!(source["judgment"]["judgment"]["review_result"], "findings");
            assert_eq!(
                source["imported_ci_digest"],
                ci.map(hash).map_or(Value::Null, Value::String)
            );
            assert_eq!(
                source["public_plan"]["plan"]["items"][1]["disposition"]["kind"],
                "summary"
            );
        }
        for f in [&f, &h.f] {
            if let Some(ci) = ci {
                assert_eq!(fs::read(f.root.path().join("ci.bin"))?, ci);
            } else {
                assert!(!f.root.path().join("ci.bin").exists());
            }
        }
    }
    Ok(())
}

#[test]
fn forged_execution_and_ci_fields_fail_through_real_file_and_replay_boundaries() -> Result<()> {
    // A single object value only detects tolerant Value decoders. Typed optional
    // fields can reject that object yet accept scalar/array authority claims.
    for (shape, forged) in [
        ("object", json!({"forged":true})),
        ("true", json!(true)),
        ("false", json!(false)),
        ("zero", json!(0)),
        ("one", json!(1)),
        ("string", json!("passed")),
        ("argv", json!(["/bin/true", "--recorded-only"])),
        ("empty-array", json!([])),
        ("null", Value::Null),
    ] {
        assert_forged_fields_reject(shape, &forged)?;
    }
    Ok(())
}

fn assert_forged_fields_reject(shape: &str, forged: &Value) -> Result<()> {
    for field in AUTHORITY_FIELDS {
        for location in [
            "intake",
            "answer",
            "plan",
            "wrapper",
            "source_view_json",
            "observation_view_json",
            "prepared_payload_binding",
        ] {
            let mut h = history(None)?;
            // An unchanged independently admitted summary/finding is the positive control.
            let original = h.f.invoke("agent-export", &[])?;
            successful(Output {
                status: original.status,
                stdout: original.stdout.clone(),
                stderr: original.stderr,
            })?;
            if ["intake", "answer", "plan"].contains(&location) {
                let prior = ["answer", "plan"].map(|key| h.f.intake[key]["sha256"].clone());
                if location == "intake" {
                    h.f.intake[field] = forged.clone();
                } else {
                    let name = format!("{location}.json");
                    let mut object: Value =
                        serde_json::from_slice(&fs::read(h.f.root.path().join(&name))?)?;
                    object[field] = forged.clone();
                    h.f.intake[location] = put(&h.f, &name, &object)?;
                    // Keep the answer->plan identity valid, so unknown-field admission is exercised.
                    if location == "answer" {
                        let mut plan: Value =
                            serde_json::from_slice(&fs::read(h.f.root.path().join("plan.json"))?)?;
                        plan["judgment_digest"] = h.f.intake["answer"]["sha256"].clone();
                        h.f.intake["plan"] = put(&h.f, "plan.json", &plan)?;
                    }
                }
                // Repin every dependent agent identity too: a stale payload or
                // observation link must not mask weakened answer/plan admission.
                for (key, old) in ["answer", "plan"].into_iter().zip(prior) {
                    let old = old
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("old pin absent"))?;
                    let new = h.f.intake[key]["sha256"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("new pin absent"))?;
                    h.subject_json = h.subject_json.replace(old, new);
                    h.payload_metadata_json = h.payload_metadata_json.replace(old, new);
                }
                h.subject = serde_json::from_str(&h.subject_json)?;
                h.f.intake["observations"]["payload_source_digest"] = json!(domain_hash(
                    "ub-review.recorded-prepared-payload-source.v1",
                    h.payload_metadata_json.as_bytes()
                ));
                h.rechain()?;
                h.admit(1)?;
                let agent_intake = h.f.intake.clone();
                for operation in ["export", "agent-export"] {
                    h.f.intake = agent_intake.clone();
                    if operation == "export" {
                        source_intake(&mut h.f);
                    }
                    let out = h.f.invoke(operation, &[])?;
                    assert!(
                        !out.status.success(),
                        "{field} ({shape}) at {location} accepted by {operation}"
                    );
                    assert!(out.stdout.is_empty());
                }
            } else {
                let mut view: Value = serde_json::from_slice(&original.stdout)?;
                if location == "wrapper" {
                    view[field] = forged.clone();
                } else if location == "prepared_payload_binding" {
                    view[location][field] = forged.clone();
                } else {
                    let mut component = decode_bytes(&view[location])?;
                    component[field] = forged.clone();
                    view[location] = json!(serde_json::to_vec(&component)?);
                }
                let bytes = serde_json::to_vec(&view)?;
                fs::write(h.f.root.path().join("forged.json"), &bytes)?;
                rejected(
                    h.f.invoke(
                        "agent-replay",
                        &["--view", "forged.json", "--view-sha256", &hash(&bytes)],
                    )?,
                    "recorded agent replay rejected",
                );
            }
        }
        // Plain source replay has a different envelope/decoder from agent replay.
        let f = Fixture::new()?;
        let original = f.invoke("export", &[])?;
        successful(Output {
            status: original.status,
            stdout: original.stdout.clone(),
            stderr: original.stderr,
        })?;
        let mut view: Value = serde_json::from_slice(&original.stdout)?;
        view[field] = forged.clone();
        let bytes = serde_json::to_vec(&view)?;
        fs::write(f.root.path().join("forged-source.json"), &bytes)?;
        rejected(
            f.invoke(
                "replay",
                &[
                    "--view",
                    "forged-source.json",
                    "--view-sha256",
                    &hash(&bytes),
                ],
            )?,
            "recorded export replay rejected",
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod effects {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn snapshot(root: &Path) -> Result<BTreeMap<String, (Vec<u8>, u32)>> {
        use std::os::unix::fs::PermissionsExt;
        fn visit(
            root: &Path,
            dir: &Path,
            files: &mut BTreeMap<String, (Vec<u8>, u32)>,
        ) -> Result<()> {
            for entry in fs::read_dir(dir)? {
                let path = entry?.path();
                let meta = fs::symlink_metadata(&path)?;
                let relative = path.strip_prefix(root)?.to_string_lossy().into_owned();
                if meta.is_dir() {
                    files.insert(relative, (vec![], meta.permissions().mode()));
                    visit(root, &path, files)?;
                } else {
                    anyhow::ensure!(meta.is_file(), "nonregular fixture path");
                    files.insert(relative, (fs::read(&path)?, meta.permissions().mode()));
                }
            }
            Ok(())
        }
        let mut files = BTreeMap::new();
        visit(root, root, &mut files)?;
        Ok(files)
    }

    /// Classify every observed syscall, rather than searching only for an expected sentinel.
    /// Unknown/truncated/resumed calls fail closed. Failed forbidden attempts count too.
    fn forbidden(trace: &str, executable: &Path) -> Vec<String> {
        let initial = format!("execve({:?},", executable.as_os_str());
        let mut entries = 0;
        let mut forbidden = vec![];
        for line in trace.lines() {
            let call = line
                .split_once(' ')
                .map(|(_, call)| call.trim_start())
                .unwrap_or_default();
            if call.starts_with("+++ exited with ") {
                continue;
            }
            let name = call.split('(').next().unwrap_or_default();
            let safe = match name {
                "execve" => {
                    entries += 1;
                    entries == 1 && call.starts_with(&initial) && call.ends_with("= 0")
                }
                "exit" | "exit_group" | "read" | "readv" | "pread64" | "close" | "fstat"
                | "lseek" | "poll" | "ppoll" => true,
                "fcntl" => call.contains("F_GETFD") || call.contains("F_GETFL"),
                "ioctl" => call.contains(", TCGETS,"),
                "access" | "faccessat" | "faccessat2" | "stat" | "lstat" | "statx"
                | "newfstatat" | "readlink" | "readlinkat" | "getcwd" => true,
                "open" | "openat" | "openat2" => {
                    call.contains("O_RDONLY")
                        && ![
                            "O_WRONLY",
                            "O_RDWR",
                            "O_CREAT",
                            "O_TRUNC",
                            "O_APPEND",
                            "O_TMPFILE",
                        ]
                        .iter()
                        .any(|flag| call.contains(flag))
                }
                "write" | "writev" => {
                    call.starts_with(&format!("{name}(1,"))
                        || call.starts_with(&format!("{name}(2,"))
                }
                "mmap" => !call.contains("PROT_WRITE") || !call.contains("MAP_SHARED"),
                _ => false,
            };
            if !safe {
                forbidden.push(line.to_owned());
            }
        }
        if entries != 1 {
            forbidden.push(format!(
                "expected exactly one entry execve; observed {entries}"
            ));
        }
        forbidden
    }

    fn trace(
        executable: &Path,
        args: &[&std::ffi::OsStr],
        cwd: &Path,
        log: &Path,
        probe: bool,
    ) -> Result<(Output, String)> {
        let mut command = Command::new("/usr/bin/strace");
        command.args(["-f", "--kill-on-exit", "-qq", "-s", "256", "-e",
            "trace=%process,%network,%file,%desc,mmap,io_uring_setup,io_uring_register,io_uring_enter",
            "-o"]).arg(log).arg(executable).args(args)
            .env_clear().current_dir(cwd).stdout(Stdio::piped()).stderr(Stdio::piped());
        if probe {
            command.env("UB_REVIEW_AUTHORITY_TRACE_PROBE", cwd);
        }
        let mut child = command.spawn()?;
        if child.wait_timeout(Duration::from_secs(20))?.is_none() {
            child.kill()?;
            child.wait()?;
            bail!("effect observer timed out");
        }
        let output = child.wait_with_output()?;
        let raw = fs::read_to_string(log)?;
        anyhow::ensure!(raw.len() <= 1024 * 1024, "effect trace exceeded 1 MiB");
        Ok((output, raw))
    }

    #[test]
    #[ignore = "trusted observer calibration child; invoked only by the effect test"]
    fn trace_probe() -> Result<()> {
        let root = std::env::var_os("UB_REVIEW_AUTHORITY_TRACE_PROBE")
            .ok_or_else(|| anyhow::anyhow!("probe root is required"))?;
        let status = Command::new("/bin/true").env_clear().status()?;
        anyhow::ensure!(status.success(), "calibration child failed");
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let _stream = std::net::TcpStream::connect(listener.local_addr()?)?;
        fs::write(
            Path::new(&root).join("observer-marker"),
            b"trusted test calibration",
        )?;
        fs::remove_file(Path::new(&root).join("observer-marker"))?;
        Ok(())
    }

    #[test]
    #[ignore = "Linux effect proof requires /usr/bin/strace; run explicitly with --ignored --exact"]
    fn recorded_commands_have_no_execution_network_or_write_effects() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let retained = std::env::var_os("UB_REVIEW_AUTHORITY_TRACE_DIR");
        let logs = retained
            .as_deref()
            .map(Path::new)
            .unwrap_or_else(|| temporary.path());
        fs::create_dir_all(logs)?;
        let probe_root = tempfile::tempdir()?;
        let probe_before = snapshot(probe_root.path())?;
        let test_binary = std::env::current_exe()?;
        let probe_name = "agent_result::authority::effects::trace_probe";
        let (probe, probe_trace) = trace(
            &test_binary,
            &[
                "--ignored".as_ref(),
                "--exact".as_ref(),
                probe_name.as_ref(),
            ],
            probe_root.path(),
            &logs.join("calibration.strace"),
            true,
        )?;
        anyhow::ensure!(
            probe.status.success(),
            "probe failed: {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        assert_eq!(snapshot(probe_root.path())?, probe_before);
        let caught = forbidden(&probe_trace, &test_binary).join("\n");
        assert!(
            caught.contains("execve(\"/bin/true\""),
            "observer missed child exec"
        );
        assert!(
            caught.contains("connect("),
            "observer missed loopback network"
        );
        assert!(
            caught
                .lines()
                .any(|line| line.contains("observer-marker") && line.contains("O_CREAT")),
            "observer missed marker creation"
        );
        assert!(
            caught
                .lines()
                .any(|line| line.contains("write(") && line.contains("trusted test calibration")),
            "observer missed non-stdout write"
        );
        assert!(
            caught
                .lines()
                .any(|line| line.contains("unlink(") && line.contains("observer-marker")),
            "observer missed marker removal"
        );

        let executable = Path::new(env!("CARGO_BIN_EXE_ub-review"));
        let mut cases = 0;
        for operation in ["export", "replay", "agent-export", "agent-replay"] {
            for shape in [
                "valid",
                "malformed",
                "rejected",
                "late_payload",
                "late_observation",
                "late_replay",
                "retry_prepared",
                "retry_superseded",
                "retry_confirmed",
            ] {
                if !operation.starts_with("agent")
                    && !["valid", "malformed", "rejected"].contains(&shape)
                    || shape == "late_replay" && operation != "agent-replay"
                {
                    continue;
                }
                let mut h = history(None)?;
                let prefix = match shape {
                    "retry_prepared" => Some(3),
                    "retry_superseded" => Some(4),
                    "retry_confirmed" => Some(5),
                    _ => None,
                };
                if let Some(count) = prefix {
                    h.retry_history()?;
                    h.admit(count)?;
                }
                if !operation.starts_with("agent") {
                    source_intake(&mut h.f);
                }
                let export = if operation.starts_with("agent") {
                    "agent-export"
                } else {
                    "export"
                };
                let out = h.f.invoke(export, &[])?;
                let view = successful(Output {
                    status: out.status,
                    stdout: out.stdout.clone(),
                    stderr: out.stderr,
                })?;
                if let Some(count) = prefix {
                    let observations = decode_bytes(&view["observation_view_json"])?;
                    assert_eq!(observations["attempts"].as_array().map(Vec::len), Some(2));
                    assert_eq!(observations["attempts"][0]["remote"]["outcome"], "unknown");
                    assert_eq!(
                        observations["attempts"][0]["local_failure"]["stage"],
                        "submission"
                    );
                    assert_eq!(observations["attempts"][1]["attempt_id"], "delivery-2");
                    assert_eq!(observations["attempts"][1]["retry_of"], "delivery-1");
                    assert!(observations["attempts"][1]["local_failure"].is_null());
                    assert_eq!(
                        observations["attempts"][1]["remote"]["outcome"],
                        if count == 5 {
                            "confirmed_inline"
                        } else {
                            "unknown"
                        }
                    );
                    assert_eq!(
                        observations["source_currentness"],
                        if count >= 4 {
                            "historical_at_recorded_watermark"
                        } else {
                            "at_recorded_source_head"
                        }
                    );
                    assert_eq!(view["whole_review_delivery"], "not_qualified");
                    assert_eq!(observations["summary_payload_correspondence"], "unverified");
                    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_SENTINEL"));
                }
                let mut view_bytes = out.stdout;
                if shape == "late_replay" {
                    // Valid JSON and matching file pin; only the final wrapper
                    // comparison can reject this forged qualification.
                    let mut forged = view;
                    forged["whole_review_delivery"] = json!("confirmed");
                    view_bytes = serde_json::to_vec(&forged)?;
                }
                if shape == "late_payload" {
                    let mut payload: Value =
                        serde_json::from_slice(&fs::read(h.f.root.path().join("payload.json"))?)?;
                    payload["body"] = json!("");
                    h.f.intake["observations"]["payload"] = put(&h.f, "payload.json", &payload)?;
                    // Keep all byte/metadata/subject/chain pins consistent so
                    // failure reaches prepared-payload semantic validation.
                    h.repin_payload_and_chain()?;
                    h.admit(1)?;
                    if operation == "agent-replay" {
                        // Observation replay precedes prepared-payload validation.
                        // Match its repinned subject and watermark exactly, so
                        // stale component bytes cannot mask the intended failure.
                        let wrapper: Value = serde_json::from_slice(&view_bytes)?;
                        let mut observations = decode_bytes(&wrapper["observation_view_json"])?;
                        observations["subject"] = h.subject.clone();
                        observations["watermark"] = serde_json::from_slice(&fs::read(
                            h.f.root.path().join("watermark.json"),
                        )?)?;
                        let mut bytes = serde_json::to_vec(&observations)?;
                        bytes.push(b'\n');
                        let mut binding = wrapper["prepared_payload_binding"].clone();
                        binding["payload_digest"] =
                            h.f.intake["observations"]["payload"]["sha256"].clone();
                        binding["payload_source_digest"] =
                            h.f.intake["observations"]["payload_source_digest"].clone();
                        // Preserve the producer's wrapper field order and each
                        // component's trailing newline for the final byte comparison.
                        let mut wire = String::from_utf8(view_bytes)?;
                        for (name, replacement) in [
                            ("observation_view_json", json!(bytes)),
                            ("prepared_payload_binding", binding),
                        ] {
                            let old =
                                format!("\"{name}\":{}", serde_json::to_string(&wrapper[name])?);
                            let new =
                                format!("\"{name}\":{}", serde_json::to_string(&replacement)?);
                            assert_eq!(wire.matches(&old).count(), 1);
                            wire = wire.replacen(&old, &new, 1);
                        }
                        view_bytes = wire.into_bytes();
                    }
                }
                if shape == "late_observation" {
                    let mut event: Value = serde_json::from_slice(&h.events[1])?;
                    event["observation"]["stage"] = json!("comment_reconciliation");
                    h.events[1] = serde_json::to_vec(&event)?;
                    h.rechain()?;
                    h.admit(2)?;
                }
                let view_pin = hash(&view_bytes);
                fs::write(h.f.root.path().join("effect-view.json"), &view_bytes)?;
                let f = &h.f;
                let intake = if shape == "malformed" {
                    b"{ invalid recorded intake".to_vec()
                } else {
                    serde_json::to_vec(&f.intake)?
                };
                fs::write(f.root.path().join("intake.json"), &intake)?;
                let pin = if shape == "rejected" {
                    "0".repeat(64)
                } else {
                    hash(&intake)
                };
                let mut args: Vec<&std::ffi::OsStr> = vec![
                    "recorded-review".as_ref(),
                    operation.as_ref(),
                    "--evidence-root".as_ref(),
                    f.root.path().as_os_str(),
                    "--intake".as_ref(),
                    "intake.json".as_ref(),
                    "--intake-sha256".as_ref(),
                    pin.as_ref(),
                ];
                if operation.ends_with("replay") {
                    args.extend::<[&std::ffi::OsStr; 4]>([
                        "--view".as_ref(),
                        "effect-view.json".as_ref(),
                        "--view-sha256".as_ref(),
                        view_pin.as_ref(),
                    ]);
                }
                let before = snapshot(f.root.path())?;
                let name = format!("{operation}-{shape}.strace");
                let (out, raw) = trace(executable, &args, f.root.path(), &logs.join(name), false)?;
                assert_eq!(
                    out.status.success(),
                    shape == "valid" || prefix.is_some(),
                    "{operation} {shape}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                if shape == "valid" || prefix.is_some() {
                    successful(out)?;
                } else {
                    assert!(out.stdout.is_empty());
                    if shape.starts_with("late_") {
                        assert!(
                            String::from_utf8_lossy(&out.stderr).contains(
                                if operation == "agent-export" {
                                    "recorded agent validation rejected"
                                } else {
                                    "recorded agent replay rejected"
                                }
                            ),
                            "late failure did not reach agent validation: {}",
                            String::from_utf8_lossy(&out.stderr)
                        );
                    }
                }
                assert_eq!(
                    snapshot(f.root.path())?,
                    before,
                    "{operation} {shape} changed source files"
                );
                let effects = forbidden(&raw, executable);
                assert!(
                    effects.is_empty(),
                    "{operation} {shape}: {}",
                    effects.join("\n")
                );
                println!(
                    "{operation} {shape}: one entry execve, zero forbidden syscall attempts, fixture bytes/modes unchanged"
                );
                cases += 1;
            }
        }
        assert_eq!(cases, 23);
        println!(
            "23 observed cases; calibration detects child execution, loopback connection and transient create/write/unlink with unchanged fixture bytes/modes. Traces: {}",
            logs.display()
        );
        Ok(())
    }
}

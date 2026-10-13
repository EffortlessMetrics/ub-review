//! Non-authoritative module-disclosure evidence (#813), independent of findings.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Result as IoResult};
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CAP: usize = 65_536;
const ENTRY_CAP: usize = 128;
const PREFIX: &str = "ripr: ";
const MARKER: &str = " Rust module composition limitation(s): ";
const SUFFIX: &str = "; affected module contexts remain fail-closed.";

const MODULE_AMBIGUOUS_PARENT_NEXT_STEP: &str = "To resolve rust_module_ambiguous_parent, give each listed file one owner: declare it from a single parent `mod` item (drop duplicate `#[path]` declarations and keep only one of `<name>.rs` or `<name>/mod.rs`); a shared `tests/<name>/mod.rs` helper needs the same `mod <name>;` declaration, with the same `#[cfg(test)]` gating, in every integration test.";

const ANALYSIS_LIMIT_ORIENTATION: &str = "This is an analysis-limit note about indexed context, not a finding: no action is needed unless evidence you expected from a listed file is missing.";

/// Read only a bounded prefix. Its digest attests only those bytes when capped.
fn read_prefix(path: &Path) -> IoResult<Vec<u8>> {
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(std::io::Error::other(
            "diagnostic source is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take((CAP + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn parse_note(line: &str) -> Option<Vec<(String, String)>> {
    let body = line.strip_prefix(PREFIX)?;
    let (count_text, body) = body.split_once(MARKER)?;
    let count = count_text.parse::<usize>().ok()?;
    if count == 0 || count > ENTRY_CAP || count.to_string() != count_text {
        return None;
    }
    let (body, orientation) = body.split_once(SUFFIX)?;
    if !orientation.is_empty()
        && orientation != format!(" {ANALYSIS_LIMIT_ORIENTATION}")
        && orientation
            != format!(" {ANALYSIS_LIMIT_ORIENTATION} {MODULE_AMBIGUOUS_PARENT_NEXT_STEP}")
    {
        return None;
    }
    let mut entries = BTreeSet::new();
    for item in body.split(", ") {
        let (context, reason) = item.rsplit_once(':')?;
        if context.is_empty()
            || context.len() > 1024
            || context.chars().any(char::is_control)
            || context.starts_with('/')
            || context.split('/').any(|part| part == "..")
            || !matches!(
                reason,
                "rust_module_ambiguous_parent"
                    | "rust_module_cycle_or_depth_limit"
                    | "rust_module_context_conflict"
                    | "rust_module_unresolved_target"
            )
        {
            return None;
        }
        if reason == "rust_module_unresolved_target" {
            let (_, location) = context.rsplit_once(':')?;
            if location.parse::<u64>().ok()? == 0 {
                return None;
            }
        }
        if !entries.insert((context.to_owned(), reason.to_owned())) {
            return None;
        }
    }
    (entries.len() == count).then(|| entries.into_iter().collect())
}

pub(super) fn module_limit_evidence(dir: &Path, detail_status: &str) -> Value {
    let mut entries: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut sources = Vec::new();
    for (name, artifact) in [
        ("stderr.txt", "sensors/ripr/stderr.txt"),
        (
            "exposure-gaps.ripr.stderr",
            "sensors/ripr/exposure-gaps.ripr.stderr",
        ),
    ] {
        let mut source = json!({"artifact": artifact});
        match read_prefix(&dir.join(name)) {
            Err(_) => {
                source["state"] = json!("unavailable");
            }
            Ok(mut bytes) => {
                let truncated = bytes.len() > CAP;
                bytes.truncate(CAP);
                source["captured_bytes"] = json!(bytes.len());
                source["truncated"] = json!(truncated);
                source["prefix_sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
                let mut state = if bytes.is_empty() {
                    "not_disclosed"
                } else {
                    "uninterpreted"
                };
                let mut accepted = 0;
                let mut rejected = 0;
                let mut uninterpreted_lines = 0;
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    for line in text.lines() {
                        if !line.starts_with(PREFIX) || !line.contains(MARKER) {
                            if !line.is_empty() {
                                uninterpreted_lines += 1;
                            }
                            continue;
                        }
                        // Never interpret a final partial line or silently drop overflowing entries.
                        let parsed = if truncated || line.len() > 8192 {
                            None
                        } else {
                            parse_note(line)
                        };
                        if let Some(parsed) = parsed {
                            let new = parsed
                                .iter()
                                .filter(|entry| !entries.contains_key(*entry))
                                .count();
                            if entries.len() + new > ENTRY_CAP {
                                rejected += 1;
                                continue;
                            }
                            accepted += 1;
                            for entry in parsed {
                                entries
                                    .entry(entry)
                                    .or_default()
                                    .insert(artifact.to_owned());
                            }
                        } else {
                            rejected += 1;
                        }
                    }
                }
                if accepted > 0 {
                    state = "disclosed";
                }
                if rejected > 0 {
                    state = "invalid_disclosure";
                }
                if truncated {
                    state = "truncated";
                }
                source["state"] = json!(state);
                source["accepted_notes"] = json!(accepted);
                source["rejected_notes"] = json!(rejected);
                source["uninterpreted_lines"] = json!(uninterpreted_lines);
                source["utf8"] = json!(std::str::from_utf8(&bytes).is_ok());
            }
        }
        sources.push(source);
    }
    let entries: Vec<Value> = entries
        .into_iter()
        .map(|((context, reason), sources)| {
            json!({
                "tool": "ripr", "context_display": context, "reason": reason,
                "source_artifacts": sources, "semantics": "analysis_limit_disclosure",
            })
        })
        .collect();
    json!({
        "schema": "ub-review.ripr_module_limit_evidence.v1",
        "parser_contract": "ripr.module-composition-stderr.v1",
        "authority": "diagnostic_only",
        "completeness": "not_established",
        "detail_status": detail_status,
        "source_byte_cap": CAP, "entry_cap": ENTRY_CAP,
        "entries": entries, "sources": sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::fs;

    fn note(context: &str) -> String {
        format!("ripr: 1{MARKER}{context}:rust_module_ambiguous_parent{SUFFIX}\n")
    }

    #[test]
    fn module_note_line_byte_boundary_excludes_line_terminators() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut contexts = (0..8)
            .map(|index| format!("{index}{}", "a".repeat(979)))
            .collect::<Vec<_>>();
        let make = |contexts: &[String]| {
            let entries = contexts
                .iter()
                .map(|context| format!("{context}:rust_module_ambiguous_parent"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("ripr: 8{MARKER}{entries}{SUFFIX}")
        };
        let padding = 8192 - make(&contexts).len();
        contexts[7].push_str(&"a".repeat(padding));
        assert!(contexts.iter().all(|context| context.len() <= 1024));
        let exact = make(&contexts);
        assert_eq!(exact.len(), 8192);
        assert_eq!(parse_note(&exact).as_ref().map(Vec::len), Some(8));
        contexts[7].push('a');
        let over = make(&contexts);
        assert_eq!(over.len(), 8193);
        assert!(contexts.iter().all(|context| context.len() <= 1024));
        assert_eq!(parse_note(&over).as_ref().map(Vec::len), Some(8));
        for ending in ["", "\n", "\r\n"] {
            for (line, accepted, rejected, count, state) in [
                (&exact, 1, 0, 8, "disclosed"),
                (&over, 0, 1, 0, "invalid_disclosure"),
            ] {
                let bytes = format!("{line}{ending}");
                fs::write(temp.path().join("stderr.txt"), &bytes)?;
                let value = module_limit_evidence(temp.path(), "detail_unavailable");
                assert_eq!(value["sources"][0]["captured_bytes"], bytes.len());
                assert_eq!(value["sources"][0]["truncated"], false);
                assert_eq!(value["sources"][0]["accepted_notes"], accepted);
                assert_eq!(value["sources"][0]["rejected_notes"], rejected);
                assert_eq!(value["sources"][0]["state"], state);
                assert_eq!(value["entries"].as_array().map(Vec::len), Some(count));
            }
        }
        Ok(())
    }

    #[test]
    fn module_note_orientation_sentence_accepts_optional_next_step() {
        let bare = note("x.rs");
        let oriented = format!("{} {ANALYSIS_LIMIT_ORIENTATION}", bare.trim());
        let expected = Some(vec![("x.rs".into(), "rust_module_ambiguous_parent".into())]);
        assert_eq!(parse_note(&oriented), expected);
        assert_eq!(
            parse_note(&format!("{oriented} {MODULE_AMBIGUOUS_PARENT_NEXT_STEP}")),
            expected
        );
        for malformed in [
            format!("{oriented} unsupported addition"),
            format!("{} {MODULE_AMBIGUOUS_PARENT_NEXT_STEP}", bare.trim()),
            oriented.replace("missing.", "missing"),
        ] {
            assert!(parse_note(&malformed).is_none(), "{malformed}");
        }
    }

    #[test]
    fn read_prefix_rejects_non_regular_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let error = read_prefix(temp.path())
            .err()
            .ok_or_else(|| anyhow::anyhow!("directory unexpectedly read as a regular file"))?;
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert_eq!(error.to_string(), "diagnostic source is not a regular file");
        Ok(())
    }

    #[test]
    fn module_note_context_limit_counts_utf8_bytes() {
        let exact_ascii = "a".repeat(1024);
        assert!(parse_note(note(&exact_ascii).trim()).is_some());
        assert!(parse_note(note(&(exact_ascii + "a")).trim()).is_none());

        let exact_utf8 = format!("{}{}", "界".repeat(256), "a".repeat(256));
        assert_eq!(exact_utf8.len(), 1024);
        assert!(parse_note(note(&exact_utf8).trim()).is_some());
        let over_utf8 = format!("{exact_utf8}界");
        assert_eq!(over_utf8.chars().count(), 513);
        assert_eq!(over_utf8.len(), 1027);
        assert!(parse_note(note(&over_utf8).trim()).is_none());
    }

    #[test]
    fn module_note_contract_rejects_malformed_counts_reasons_and_paths() {
        assert_eq!(
            parse_note(note("tests/日本語.rs").trim()),
            Some(vec![(
                "tests/日本語.rs".into(),
                "rust_module_ambiguous_parent".into()
            )])
        );
        let oriented = format!(
            "{} {ANALYSIS_LIMIT_ORIENTATION} {MODULE_AMBIGUOUS_PARENT_NEXT_STEP}",
            note("x.rs").trim()
        );
        assert!(parse_note(&oriented).is_some());
        assert!(parse_note(&(oriented + " unsupported addition")).is_none());
        for bad in [
            note("../secret.rs"),
            note("/absolute.rs"),
            note(""),
            note("tests/control\t.rs"),
            note(&"界".repeat(350)),
            note("x.rs").replace("ripr: 1", "ripr: 2"),
            note("x.rs").replace("ripr: 1", "ripr: 01"),
            note("x.rs").replace("ambiguous_parent", "new_reason"),
            note("x.rs").replace(SUFFIX, "; different suffix"),
            note("x.rs") + "extra",
        ] {
            assert!(parse_note(bad.trim()).is_none(), "{bad:?}");
        }
        assert!(parse_note("ripr: 1 Rust module composition limitation(s): src/a.rs:12:rust_module_unresolved_target; affected module contexts remain fail-closed.").is_some());
    }

    #[test]
    fn unresolved_target_location_is_required_and_positive() {
        let make = |context: &str| {
            format!("ripr: 1{MARKER}{context}:rust_module_unresolved_target{SUFFIX}")
        };
        assert_eq!(
            parse_note(&make("src/a.rs:12")),
            Some(vec![(
                "src/a.rs:12".into(),
                "rust_module_unresolved_target".into()
            )])
        );
        for context in [
            "src/a.rs",
            "src/a.rs:0",
            "src/a.rs:not_a_number",
            "src/a.rs:18446744073709551616",
        ] {
            assert!(parse_note(&make(context)).is_none(), "{context}");
        }
    }

    #[test]
    fn module_evidence_retains_single_pass_duplicates_benign_and_unavailable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path();
        let missing = module_limit_evidence(dir, "detail_unavailable");
        assert_eq!(missing["sources"][0]["state"], "unavailable");
        for (primary, detail, count) in [
            ("", "", 0),
            ("benign\n", "", 0),
            ("note", "", 1),
            ("", "note", 1),
            ("note", "note", 1),
            ("duplicate", "note", 1),
        ] {
            let text = |value| match value {
                "note" => note("tests/common/cli_lock.rs"),
                "duplicate" => note("tests/common/cli_lock.rs").repeat(2),
                _ => value.to_owned(),
            };
            fs::write(dir.join("stderr.txt"), text(primary))?;
            fs::write(dir.join("exposure-gaps.ripr.stderr"), text(detail))?;
            let value = module_limit_evidence(dir, "ok");
            assert_eq!(value["entries"].as_array().map(Vec::len), Some(count));
            assert_eq!(value["completeness"], "not_established");
            if primary == "note" && detail == "note" {
                assert_eq!(
                    value["entries"][0]["source_artifacts"]
                        .as_array()
                        .map(Vec::len),
                    Some(2)
                );
            }
        }
        fs::write(
            dir.join("stderr.txt"),
            note("x.rs").replace("ripr: 1", "ripr: 2"),
        )?;
        fs::write(dir.join("exposure-gaps.ripr.stderr"), "")?;
        assert_eq!(
            module_limit_evidence(dir, "ok")["sources"][0]["state"],
            "invalid_disclosure"
        );
        fs::write(dir.join("stderr.txt"), vec![b'x'; CAP + 1])?;
        let value = module_limit_evidence(dir, "detail_unavailable");
        assert_eq!(value["sources"][0]["state"], "truncated");
        assert_eq!(value["sources"][0]["captured_bytes"], CAP);
        assert_eq!(value["detail_status"], "detail_unavailable");

        let mut exact_cap = note("exact-cap.rs").into_bytes();
        while exact_cap.len() + 2 <= CAP {
            exact_cap.extend_from_slice(b"x\n");
        }
        if exact_cap.len() < CAP {
            exact_cap.push(b'x');
        }
        assert_eq!(exact_cap.len(), CAP);
        fs::write(dir.join("stderr.txt"), &exact_cap)?;
        let exact = module_limit_evidence(dir, "ok");
        assert_eq!(exact["sources"][0]["captured_bytes"], CAP);
        assert_eq!(exact["sources"][0]["truncated"], false);
        assert_eq!(exact["sources"][0]["state"], "disclosed");
        assert_eq!(exact["entries"].as_array().map(Vec::len), Some(1));

        exact_cap.push(b'x');
        fs::write(dir.join("stderr.txt"), &exact_cap)?;
        let over = module_limit_evidence(dir, "ok");
        assert_eq!(over["sources"][0]["captured_bytes"], CAP);
        assert_eq!(over["sources"][0]["truncated"], true);
        assert_eq!(over["sources"][0]["state"], "truncated");
        assert_eq!(over["entries"].as_array().map(Vec::len), Some(0));
        let many = (0..129)
            .map(|index| note(&format!("src/{index}.rs")))
            .collect::<String>();
        fs::write(dir.join("stderr.txt"), many)?;
        let capped = module_limit_evidence(dir, "ok");
        assert_eq!(capped["entries"].as_array().map(Vec::len), Some(128));
        assert_eq!(capped["sources"][0]["state"], "invalid_disclosure");
        assert_eq!(capped["sources"][0]["rejected_notes"], 1);
        fs::write(
            dir.join("stderr.txt"),
            note("x.rs") + "benign unrelated line\n",
        )?;
        assert_eq!(
            module_limit_evidence(dir, "ok")["sources"][0]["uninterpreted_lines"],
            1
        );
        fs::write(dir.join("stderr.txt"), [255])?;
        assert_eq!(
            module_limit_evidence(dir, "ok")["sources"][0]["state"],
            "uninterpreted"
        );
        Ok(())
    }
}

use super::{
    ClippyDiagnostic, PrecommitStream, collect_clippy_finding, read_precommit_stream,
    write_markdown,
};
use anyhow::Result;
use serde_json::json;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::Path;

// Framing and evidence budgets are independent of the display head/tail.
// Non-finding Cargo messages cannot spend the space reserved for diagnostics.
const RECORD_BYTES: usize = 256 * 1024;
const EVIDENCE_BYTES: usize = 1024 * 1024;

struct ObservedReader<R, F> {
    reader: R,
    observe: F,
}

impl<R: Read, F: FnMut(&[u8])> Read for ObservedReader<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.reader.read(buf)?;
        (self.observe)(&buf[..n]);
        Ok(n)
    }
}

#[derive(Default)]
pub(super) struct ClippyOutput {
    messages: Vec<u8>,
    changed_bytes: usize,
    other_bytes: usize,
    omitted_messages: usize,
    findings: usize,
    details: String,
    omitted_details: usize,
    oversized: usize,
    partial: usize,
    malformed: usize,
}

impl ClippyOutput {
    pub(super) fn read(
        &mut self,
        reader: impl Read,
        root: &Path,
        package: &str,
        changed: &BTreeMap<String, BTreeSet<u64>>,
        head: usize,
        tail: usize,
    ) -> io::Result<PrecommitStream> {
        let mut line = Vec::new();
        let mut skipping = false;
        let observed = ObservedReader {
            reader,
            observe: |chunk: &[u8]| {
                for part in chunk.split_inclusive(|byte| *byte == b'\n') {
                    let (bytes, ended) = match part.strip_suffix(b"\n") {
                        Some(bytes) => (bytes, true),
                        None => (part, false),
                    };
                    if !skipping {
                        if bytes.len() > RECORD_BYTES - line.len() {
                            self.oversized += 1;
                            skipping = true;
                            line.clear();
                        } else {
                            line.extend_from_slice(bytes);
                        }
                    }
                    if ended {
                        self.record(&line, root, package, changed);
                        line.clear();
                        skipping = false;
                    }
                }
            },
        };
        let stream = read_precommit_stream(observed, head, tail)?;
        // A final valid JSON value is complete even without a trailing newline.
        self.record(&line, root, package, changed);
        Ok(stream)
    }

    fn record(
        &mut self,
        line: &[u8],
        root: &Path,
        package: &str,
        changed: &BTreeMap<String, BTreeSet<u64>>,
    ) {
        if line.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        let before = self.findings;
        let raw = match serde_json::from_slice(line) {
            Ok(value) => {
                collect_clippy_finding(root, package, &value, changed, |finding| {
                    self.finding(finding)
                });
                Cow::Borrowed(line)
            }
            Err(error) => {
                if error.is_eof() {
                    self.partial += 1;
                } else {
                    self.malformed += 1;
                }
                Cow::Owned(
                    json!({ "text": String::from_utf8_lossy(line) })
                        .to_string()
                        .into_bytes(),
                )
            }
        };
        let used = if self.findings == before {
            &mut self.other_bytes
        } else {
            &mut self.changed_bytes
        };
        let cost = raw.len() + 2; // Include the JSON array separator.
        if cost <= EVIDENCE_BYTES - *used {
            *used += cost;
            self.append_message(&raw);
        } else {
            self.omitted_messages += 1;
        }
    }

    fn append_message(&mut self, raw: &[u8]) {
        if self.messages.is_empty() {
            self.messages.push(b'[');
        } else {
            self.messages.extend_from_slice(b",\n");
        }
        self.messages.extend_from_slice(raw);
    }

    fn finding(&mut self, finding: ClippyDiagnostic) {
        self.findings += 1;
        let detail = format!(
            "- {}:{} [{}] {} ({})\n",
            finding.path, finding.line, finding.level, finding.message, finding.package
        );
        if detail.len() <= EVIDENCE_BYTES - self.details.len() {
            self.details.push_str(&detail);
        } else {
            self.omitted_details += 1;
        }
    }

    pub(super) fn incomplete(&self) -> usize {
        self.oversized + self.partial + self.malformed
    }

    fn blocking_failures(&self) -> usize {
        self.findings + self.incomplete()
    }

    pub(super) fn write_artifacts(mut self, dir: &Path) -> Result<usize> {
        if self.omitted_messages != 0 || self.incomplete() != 0 {
            let summary = json!({
                "omitted_messages": self.omitted_messages,
                "incomplete_records": self.incomplete(),
                "partial_records": self.partial,
                "malformed_records": self.malformed,
                "oversized_records": self.oversized,
            });
            self.append_message(summary.to_string().as_bytes());
        }
        if self.messages.is_empty() {
            self.messages.push(b'[');
        }
        self.messages.extend_from_slice(b"]\n");
        fs::write(dir.join("clippy.json"), &self.messages)?;
        let mut text = String::from(
            "# Clippy on diff\n\nClippy ran at affected package/target granularity. Warnings and errors with a primary span on a changed line block precommit.\n\n",
        );
        if self.incomplete() != 0 {
            text.push_str(&format!(
                "Incomplete Clippy evidence: {} oversized, {} partial and {} malformed JSON records.\n\n",
                self.oversized,
                self.partial, self.malformed
            ));
        }
        if self.findings != 0 {
            text.push_str("## Blocking diagnostics\n\n");
            text.push_str(&self.details);
        } else if self.incomplete() == 0 {
            text.push_str("No Clippy diagnostics touched changed lines.\n");
        }
        if self.omitted_details != 0 {
            text.push_str(&format!(
                "{} additional changed-line diagnostics omitted from this receipt.\n",
                self.omitted_details
            ));
        }
        write_markdown(&dir.join("clippy-on-diff.md"), &text)?;
        Ok(self.blocking_failures())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(line: u64) -> String {
        json!({"reason": "compiler-message", "message": {
            "message": "fixture warning", "level": "warning",
            "spans": [{"file_name": "src/lib.rs", "is_primary": true, "line_start": line}]
        }})
        .to_string()
    }

    #[test]
    fn changed_record_survives_display_elision() -> Result<()> {
        let input = format!(
            "{{\"head\":1}}\n{}\n{}\n{{\"tail\":2}}\n",
            diagnostic(7),
            diagnostic(8)
        );
        let changed = BTreeMap::from([("src/lib.rs".into(), BTreeSet::from([7]))]);
        let mut output = ClippyOutput::default();
        let stream = output.read(input.as_bytes(), Path::new("."), "pkg", &changed, 5, 4)?;
        assert_eq!(stream.bytes, input.len() as u64);
        assert!(stream.truncated);
        assert_eq!(
            stream.text,
            format!(
                "{{\"hea\n[... {} bytes truncated by the precommit capture budget ...]\n:2}}\n",
                input.len() - 9
            )
        );
        assert_eq!(output.blocking_failures(), 1);
        assert_eq!(
            output.details,
            "- src/lib.rs:7 [warning] fixture warning (pkg)\n"
        );
        let dir = crate::tests::temp_repo_root("clippy-middle")?;
        assert_eq!(output.write_artifacts(&dir)?, 1);
        let messages: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(dir.join("clippy.json"))?)?;
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[1]["message"]["spans"][0]["line_start"], 7);
        assert_eq!(messages[2]["message"]["spans"][0]["line_start"], 8);
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn oversized_and_partial_records_are_incomplete_and_framing_recovers() -> Result<()> {
        let mut output = ClippyOutput::default();
        let changed = BTreeMap::from([("src/lib.rs".into(), BTreeSet::from([7]))]);
        // Exactly at the limit, the JSON value is valid. One byte over, even
        // a suffix that is a diagnostic must be skipped to the next newline.
        let exact = format!("\"{}\"", "x".repeat(RECORD_BYTES - 2));
        let input = format!(
            "{exact}\n{exact} {}\n{}\r\ninvalid\n{{\"unfinished\":",
            diagnostic(7),
            diagnostic(7)
        );
        output.read(input.as_bytes(), Path::new("."), "pkg", &changed, 3, 2)?;
        assert_eq!(
            (output.oversized, output.incomplete(), output.findings),
            (1, 3, 1)
        );
        let dir = crate::tests::temp_repo_root("clippy-incomplete")?;
        assert_eq!(output.write_artifacts(&dir)?, 4);
        let messages: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(dir.join("clippy.json"))?)?;
        assert_eq!(messages.len(), 5);
        assert_eq!(
            messages[4],
            json!({"omitted_messages":0,"incomplete_records":3,"oversized_records":1,"partial_records":1,"malformed_records":1})
        );
        let markdown = fs::read_to_string(dir.join("clippy-on-diff.md"))?;
        assert!(markdown.contains("1 oversized, 1 partial and 1 malformed"));
        assert!(markdown.contains("src/lib.rs:7"));
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn receipt_limits_preserve_changed_diagnostic_counts() -> Result<()> {
        let changed = BTreeMap::from([("src/lib.rs".into(), BTreeSet::from([7]))]);
        let mut output = ClippyOutput::default();
        let padding = format!("{}\"\"", " ".repeat(RECORD_BYTES - 2));
        for _ in 0..5 {
            output.read(padding.as_bytes(), Path::new("."), "pkg", &changed, 3, 2)?;
        }
        assert_eq!(output.omitted_messages, 2);
        assert_eq!(output.incomplete(), 0);
        let mut message: serde_json::Value = serde_json::from_str(&diagnostic(7))?;
        message["message"]["message"] = "x".repeat(200_000).into();
        let input = message.to_string();
        for _ in 0..6 {
            output.read(input.as_bytes(), Path::new("."), "pkg", &changed, 3, 2)?;
        }
        assert_eq!((output.omitted_messages, output.omitted_details), (3, 1));
        assert_eq!(output.blocking_failures(), 6);
        assert!(output.messages.len() <= 2 * EVIDENCE_BYTES);
        assert!(output.details.len() <= EVIDENCE_BYTES);
        Ok(())
    }
}

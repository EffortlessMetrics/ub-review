//! Enforce the bounded source preparation for #1465's queue-only cutover.
//!
//! The two protected legacy routes remain explicit until actual-G identity,
//! immutable baseline authority, publisher and effective protection are joined.
//! Job-level skips do not make a subscribed development event allocation-free.

use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn events(source: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let mut result = BTreeMap::new();
    let mut active = false;
    let mut found = false;
    let mut current: Option<String> = None;
    for raw in source.lines() {
        let line = raw.split(" #").next().unwrap_or(raw).trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') {
            active = false;
            current = None;
            let key = line.split_once(':').map(|(key, _)| key);
            ensure!(
                matches!(
                    key,
                    Some(
                        "name"
                            | "run-name"
                            | "on"
                            | "permissions"
                            | "env"
                            | "defaults"
                            | "concurrency"
                            | "jobs"
                    )
                ),
                "workflow root keys must be static unquoted names"
            );
            if line == "on:" {
                ensure!(!found, "duplicate event mapping");
                found = true;
                active = true;
            } else if key == Some("on") {
                bail!("events must use an explicit block-style mapping");
            }
            continue;
        }
        if !active {
            continue;
        }
        if line.starts_with("  ") && !line.starts_with("   ") {
            let name = line
                .trim()
                .strip_suffix(':')
                .context("event must be an explicit block key")?;
            ensure!(
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "event name must be static and unquoted"
            );
            ensure!(
                result.insert(name.to_owned(), Vec::new()).is_none(),
                "duplicate event {name}"
            );
            current = Some(name.to_owned());
        } else if line.starts_with("    ") {
            let owner = current
                .as_ref()
                .context("event configuration lacks its event")?;
            result
                .get_mut(owner)
                .context("missing event owner")?
                .push(line.trim().to_owned());
        } else {
            bail!("event indentation is not auditable");
        }
    }
    ensure!(
        found && !result.is_empty(),
        "workflow needs an explicit nonempty event mapping"
    );
    Ok(result)
}

fn validate_source(name: &str, source: &str) -> Result<()> {
    for (event, configuration) in events(source)? {
        let allowed = match event.as_str() {
            "workflow_call" | "workflow_dispatch" => true,
            "merge_group" => configuration == ["types: [checks_requested]"],
            // Installed full proof remains authoritative during preparation.
            "pull_request" => {
                name == "ub-review-gate.yml"
                    && configuration == ["types: [opened, reopened, ready_for_review, synchronize]"]
            }
            "pull_request_target" => {
                name == "independent-baseline.yml"
                    && configuration == ["types: [opened, reopened, ready_for_review, synchronize]"]
            }
            "push" => name == "release-binary.yml" && configuration == ["tags:", "- 'v*'"],
            "schedule" => {
                name == "quality-backfill.yml" && configuration == ["- cron: '17 8 * * *'"]
            }
            _ => false,
        };
        ensure!(
            allowed,
            "{name}: {event} allocates outside the two legacy qualification routes or named release/telemetry purposes; development checks and review belong in the owning session (#1465)"
        );
    }
    Ok(())
}

pub(crate) fn validate(root: &Path) -> Result<()> {
    let mut count = 0;
    for entry in fs::read_dir(root.join(".github/workflows"))? {
        let path = entry?.path();
        if !matches!(
            path.extension().and_then(|v| v.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .context("workflow name must be UTF-8")?;
        validate_source(name, &fs::read_to_string(&path)?)
            .with_context(|| format!("workflow lifecycle: {}", path.display()))?;
        count += 1;
    }
    ensure!(count > 0, "workflow lifecycle audit inspected no workflows");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow(event: &str, fields: &str) -> String {
        format!(
            "name: fixture\non:\n  {event}:\n{fields}jobs:\n  skipped:\n    if: false\n    runs-on: ubuntu-latest\n    steps:\n      - run: ':'\n"
        )
    }

    #[test]
    fn cheap_or_skipped_wrappers_do_not_admit_development_events() {
        for event in [
            "pull_request",
            "pull_request_target",
            "pull_request_review",
            "pull_request_review_comment",
            "issue_comment",
            "issues",
            "push",
            "workflow_run",
            "repository_dispatch",
        ] {
            assert!(
                validate_source("new-wrapper.yml", &workflow(event, "")).is_err(),
                "{event}"
            );
        }
    }

    #[test]
    fn legacy_routes_cannot_expand_or_move() {
        let fields = "    types: [opened, reopened, ready_for_review, synchronize]\n";
        for (name, event) in [
            ("ub-review-gate.yml", "pull_request"),
            ("independent-baseline.yml", "pull_request_target"),
        ] {
            assert!(validate_source(name, &workflow(event, fields)).is_ok());
            assert!(validate_source("copy.yml", &workflow(event, fields)).is_err());
            assert!(validate_source(name, &workflow(event, "")).is_err());
            assert!(
                validate_source(
                    name,
                    &workflow(event, "    types: [opened, labeled, synchronize]\n")
                )
                .is_err()
            );
        }
    }

    #[test]
    fn only_named_release_and_telemetry_events_survive() {
        assert!(
            validate_source(
                "release-binary.yml",
                &workflow("push", "    tags:\n      - 'v*'\n")
            )
            .is_ok()
        );
        assert!(
            validate_source(
                "release-binary.yml",
                &workflow("push", "    branches: [main]\n")
            )
            .is_err()
        );
        assert!(
            validate_source(
                "quality-backfill.yml",
                &workflow("schedule", "    - cron: '17 8 * * *'\n")
            )
            .is_ok()
        );
        for name in ["renamed-main-health.yml", "new-maintenance.yml"] {
            assert!(
                validate_source(name, &workflow("schedule", "    - cron: '17 8 * * *'\n")).is_err()
            );
        }
        assert!(
            validate_source(
                "quality-backfill.yml",
                &workflow("schedule", "    - cron: '* * * * *'\n")
            )
            .is_err()
        );
    }

    #[test]
    fn queue_requires_actual_checks_requested_and_explicit_work_remains_available() {
        assert!(
            validate_source(
                "quality-bootstrap-tests.yml",
                &workflow("merge_group", "    types: [checks_requested]\n")
            )
            .is_ok()
        );
        assert!(
            validate_source("quality-bootstrap-tests.yml", &workflow("merge_group", "")).is_err()
        );
        assert!(validate_source("incident.yml", &workflow("workflow_dispatch", "")).is_ok());
    }

    #[test]
    fn alternate_yaml_spellings_cannot_hide_development_events() {
        for source in [
            "on: push\n",
            "on: [pull_request]\n",
            "on: {pull_request: {}}\n",
            "on: *events\n",
            "\"on\":\n  pull_request:\n",
            "on:\n  workflow_dispatch:\n'on':\n  pull_request:\n",
            "on:\n  workflow_dispatch:\non :\n  pull_request:\n",
            "on:\n  workflow_dispatch:\n  workflow_dispatch:\n",
            "on:\n  'pull_request':\n",
        ] {
            assert!(validate_source("new.yml", source).is_err(), "{source}");
        }
    }

    #[test]
    fn actual_repository_catalog_is_checked() -> Result<()> {
        validate(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .context("workspace root")?,
        )
    }
}
